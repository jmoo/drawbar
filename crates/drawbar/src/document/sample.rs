//! The sample-instrument document.
//!
//! A sample is mostly encoded audio, so what is settable is what the format can patch in
//! place without touching a stroke: the name, each zone's root key and boundaries, and
//! the v2 keyboard map's per-key gain and detune. Every generation edits. Where a
//! container also describes the keyboard note by note, that description is recomputed
//! from the zones as they move.
//!
//! ⚠️ Decoding a stroke is expensive and a library instrument is hundreds of megabytes,
//! so **nothing here decodes to draw a frame**. A zone's audio is decoded once, when the
//! operator asks for it, and kept in a [`Cache`] until the bytes under it change.
//!
//! The chrome the project editor shares — the key map, the zone rows, the cells of an
//! open row — lives here rather than being written twice: an `.nsmpproj` is the same
//! object seen from the source side.

use std::collections::{BTreeSet, HashMap};
use std::io::Cursor;

use eframe::egui;
use nord_format::formats::nsmp::codec::{self, Audio};
use nord_format::formats::nsmp::{keymap, zone, KeyTable, Level, Sty};
use nord_format::{Entity, Sample};

use super::capability::{Fact, Offset, Row, State as Cap};
use super::controls::{self, Sets};
use super::header::{Body, Cell};
use super::keys;
use crate::app;
use crate::icon::{icon, Glyph};
use crate::note;
use crate::room;

pub fn is_sample(entity: &Entity) -> bool {
    matches!(entity, Entity::Sample(_))
}

fn sample(entity: &Entity) -> Option<&Sample> {
    match entity {
        Entity::Sample(sample) => Some(sample),
        _ => None,
    }
}

fn sample_mut(entity: &mut Entity) -> Option<&mut Sample> {
    match entity {
        Entity::Sample(sample) => Some(sample),
        _ => None,
    }
}

/// One zone, numbered the way the panel numbers them: from 1, in stored order.
#[derive(Clone, PartialEq, Eq)]
pub struct Zone {
    pub root_key: u8,
    pub top_note: u8,
    /// The bottom of the range where the file states one outright. The v2 table does
    /// not, so there it is worked out from the zone below.
    pub low_note: Option<u8>,
    /// The zone record's own linear gain, where the generation carries one —
    /// [`zone::GAIN_UNITY`] is 1.0. Nothing here writes it.
    pub gain: Option<u32>,
    /// The velocity window the record states, where the layout has one. Nothing here
    /// writes it either.
    pub velocity: Option<(u8, u8)>,
    /// The stroke stream this zone plays, in bytes.
    pub bytes: usize,
}

/// Everything the document shows about an instrument, in one read.
#[derive(Clone, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub max_name_len: usize,
    /// The v3/v4 second name — what follows the `_` in the vendor's filenames. Empty
    /// on a v2 instrument, which has one name.
    pub sub_name: String,
    /// `v2`, `v3` or `v4`, taken from the content version rather than the filename.
    pub generation: &'static str,
    pub categories: Vec<String>,
    pub zones: Vec<Zone>,
    /// Whether the zone controls do anything. The name is always settable.
    pub zones_editable: bool,
    /// The keyboard map ahead of the v2 zone table, where the section holds one.
    pub key_table: Option<KeyTable>,
    /// Bytes per zone record on this body's own chain, for the Advanced face.
    pub record_len: usize,
    /// What the `sty` preset states. Read only: it is the preset the loader installs
    /// for the instrument's category.
    pub sound: Vec<(&'static str, String)>,
    /// The content version, which is what decides the generation.
    pub version: u32,
}

pub fn snapshot(entity: &Entity) -> Option<Result<Snapshot, String>> {
    Some(read(sample(entity)?))
}

fn read(sample: &Sample) -> Result<Snapshot, String> {
    // Only the wide chain carries a second name, and the `cat` section this reader
    // decodes is the narrow one's.
    let (sub_name, categories) = match sample {
        Sample::V2(body) => (String::new(), body.categories()),
        Sample::V3(body) => (body.sub_name().map_err(|e| e.to_string())?, Vec::new()),
    };
    let records = match sample {
        // A `map` version with no zone layout is a body whose zones did not read at
        // all, and the error the zone read gives is the one worth showing.
        Sample::V2(body) => body
            .chain()
            .map_or(zone::RECORD_LEN, |chain| chain.zone_record_len()),
        Sample::V3(body) => body
            .zone_table()
            .map_or(zone::RECORD_LEN, |table| table.wide.record_len()),
    };
    let gains: Vec<Option<u32>> = match sample {
        Sample::V2(body) => body
            .zones()
            .map(|zones| zones.iter().map(|zone| Some(zone.gain)).collect())
            .unwrap_or_default(),
        Sample::V3(_) => Vec::new(),
    };
    let windows: Vec<Option<(u8, u8)>> = match sample {
        Sample::V2(_) => Vec::new(),
        Sample::V3(body) => body
            .zones()
            .map(|zones| {
                zones
                    .iter()
                    .map(|zone| zone.velocity.map(|window| (window.low, window.high)))
                    .collect()
            })
            .unwrap_or_default(),
    };
    Ok(Snapshot {
        name: sample.name().map_err(|e| e.to_string())?,
        max_name_len: sample.max_name_len(),
        sub_name,
        generation: sample.generation(),
        categories,
        zones: sample
            .zones()
            .map_err(|e| e.to_string())?
            .iter()
            .enumerate()
            .map(|(index, zone)| Zone {
                root_key: zone.root_key,
                top_note: zone.top_note,
                low_note: zone.low_note,
                gain: gains.get(index).copied().flatten(),
                velocity: windows.get(index).copied().flatten(),
                bytes: zone.stream.len(),
            })
            .collect(),
        zones_editable: sample.zones_are_editable(),
        key_table: match sample {
            Sample::V2(body) => body.key_table().ok(),
            Sample::V3(_) => None,
        },
        record_len: records,
        sound: sound(sample),
        version: match sample {
            Sample::V2(body) => body.header.version,
            Sample::V3(body) => body.header.version,
        },
    })
}

/// What the `sty` section states, in the words the read cells print.
///
/// The v2 preset carries the two velocity depths on a three-step scale; the wide ones
/// carry the dynamics curve and its response instead.
fn sound(sample: &Sample) -> Vec<(&'static str, String)> {
    let on = |flag: bool| match flag {
        true => "on".to_string(),
        false => "off".to_string(),
    };
    match sample {
        Sample::V2(body) => match body.sty() {
            Ok(sty) => vec![
                ("Dynamics", on(sty.dynamics_enabled())),
                (
                    "Velocity → amplitude",
                    sty.velocity_to_amplitude().to_string(),
                ),
                ("Velocity → timbre", sty.velocity_to_timbre().to_string()),
            ],
            Err(_) => Vec::new(),
        },
        Sample::V3(body) => match body.sty() {
            Ok(Sty::V3(sty)) => vec![
                ("Dynamics", on(sty.dynamics_enabled())),
                ("Dynamics curve", sty.dynamics_curve().to_string()),
                ("Dynamics response", sty.dynamics_response().to_string()),
            ],
            Ok(Sty::V4(sty)) => vec![
                ("Dynamics", on(sty.dynamics_enabled())),
                (
                    "Dynamics curve",
                    match sty.dynamics_curve() {
                        Some(curve) => curve.to_string(),
                        None => "none".to_string(),
                    },
                ),
            ],
            Ok(Sty::V2(_)) | Err(_) => Vec::new(),
        },
    }
}

/// Apply one `path = value`. Paths are the CLI's: `name`, `zone1.root_key`,
/// `zone1.top_note`, `zone1.low_note`.
fn set(sample: &mut Sample, path: &str, value: &str) -> Result<(), String> {
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
        "low_note" => sample.set_zone_low_note(index - 1, note),
        _ => return Err(unknown()),
    }
    .map_err(|e| e.to_string())
}

/// The note and field of a `key60.gain` path.
fn key_path(path: &str) -> Option<(u8, &str)> {
    let (key, field) = path.split_once('.')?;
    let note = key.strip_prefix("key")?.parse::<u8>().ok()?;
    (usize::from(note) < keymap::KEYS).then_some((note, field))
}

/// A per-key value as the lane writes it: a figure, and the unit it is measured in.
fn measured(value: &str, unit: &str) -> Result<f64, String> {
    let text = value.trim();
    text.strip_suffix(unit)
        .unwrap_or(text)
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("{value:?} is not a reading in {unit}"))
}

/// A gain field as decibels. The field is a linear ratio with unity at
/// [`keymap::GAIN_UNITY`], so the reading is the usual 20·log10 of it.
pub fn gain_db(gain: u32, unity: u32) -> f64 {
    match gain {
        0 => f64::NEG_INFINITY,
        held => 20.0 * (f64::from(held) / f64::from(unity)).log10(),
    }
}

/// The gain field that reads as `db`, refused where it does not fit the 24 bits.
fn gain_units(db: f64) -> Result<u32, String> {
    let units = f64::from(keymap::GAIN_UNITY) * 10.0_f64.powf(db / 20.0);
    let rounded = units.round();
    match rounded.is_finite() && rounded >= 0.0 && rounded <= f64::from(keymap::GAIN_MAX) {
        true => Ok(rounded as u32),
        false => Err(format!(
            "{db} dB is outside what the gain field holds (up to {:.1} dB)",
            gain_db(keymap::GAIN_MAX, keymap::GAIN_UNITY)
        )),
    }
}

/// A detune field as cents, and the field that reads as a number of them.
pub fn detune_cents(detune: i32) -> f64 {
    f64::from(detune) / f64::from(keymap::DETUNE_PER_SEMITONE) * 100.0
}

fn detune_units(cents: f64) -> i32 {
    (cents / 100.0 * f64::from(keymap::DETUNE_PER_SEMITONE)).round() as i32
}

/// Every `key{note}.{gain|detune}` set, applied to one read of the keyboard map and
/// written back once.
///
/// The map is one field: two key edits are one write, and a key's other half is carried
/// over rather than reset to neutral.
fn apply_keys(sample: &mut Sample, sets: &[(String, String)]) -> Result<(), String> {
    let Sample::V2(body) = sample else {
        return Err("only a v2 instrument carries a keyboard map this editor writes".into());
    };
    let mut table = body.key_table().map_err(|e| e.to_string())?;
    for (path, value) in sets {
        let (note, field) = key_path(path).ok_or_else(|| format!("unknown field {path:?}"))?;
        let held = table.key(note).map_err(|e| e.to_string())?;
        let level = match field {
            "gain" => Level::new(gain_units(measured(value, "dB")?)?, held.detune()),
            "detune" => Level::new(held.gain(), detune_units(measured(value, "c")?)),
            _ => return Err(format!("unknown field {path:?}")),
        }
        .map_err(|e| e.to_string())?;
        table.set_key(note, level).map_err(|e| e.to_string())?;
    }
    body.set_key_table(&table).map_err(|e| e.to_string())
}

/// Apply every set to a fresh decode and re-encode, the same all-or-nothing rule the
/// registry bodies follow.
pub fn apply(bytes: &[u8], sets: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut entity =
        nord_format::from_stream(&mut Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let sample = sample_mut(&mut entity).ok_or("not a sample instrument")?;
    let (table, fields): (Sets, Sets) = sets
        .iter()
        .cloned()
        .partition(|(path, _)| key_path(path).is_some());
    for (path, value) in &fields {
        set(sample, path, value)?;
    }
    if !table.is_empty() {
        apply_keys(sample, &table)?;
    }
    nord_format::to_bytes(&entity).map_err(|e| e.to_string())
}

/// The range a zone covers, in plain words.
///
/// Zones are stored high to low and the panel numbers them from 1 at the top of the
/// keyboard; a zone's bottom is one note above the next record's top, except where the
/// file states the bottom itself.
pub fn range(zones: &[Zone], index: usize) -> String {
    let top = note::name(zones[index].top_note);
    match bottom(zones, index) {
        Some(low) => format!("{} up to {top}", note::name(low)),
        None => format!("up to {top}"),
    }
}

/// Where a zone's range starts: what the record states, or one above the zone below.
fn bottom(zones: &[Zone], index: usize) -> Option<u8> {
    zones[index].low_note.or_else(|| {
        zones
            .get(index + 1)
            .map(|below| below.top_note.saturating_add(1))
    })
}

/// What the sample view asked the document to do about one zone's audio.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ask {
    /// Decode this zone, because the operator opened it.
    Decode(usize),
    /// Start it, or stop it if it is the one sounding.
    Play(usize),
    Save(usize),
    /// Sound this zone at the pitch a struck key asks of it.
    Strike {
        zone: usize,
        semitones: i16,
    },
}

/// Decoded zone audio, kept only while the bytes it came from are the current ones.
///
/// ⚠️ Keyed by the asset's [`stamp`](crate::workspace::LocalEntity::stamp) as well as
/// its id: an edit re-encodes the whole file, and audio decoded from what it held
/// before is audio from another instrument.
#[derive(Default)]
pub struct Cache {
    of: Option<(u64, u64)>,
    zones: HashMap<usize, Result<Decoded, String>>,
}

/// One zone's audio, and the envelope drawn from it.
pub struct Decoded {
    pub audio: Audio,
    /// Min and max per column, in `-1.0..=1.0`. Drawn once and stretched to whatever
    /// width the panel has, because rebuilding it on a window drag is another pass over
    /// every sample.
    pub envelope: Vec<(f32, f32)>,
}

/// Columns an envelope is reduced to. Wide enough that a wide panel has no gaps in it,
/// small enough that the whole thing is a few kilobytes whatever the zone holds.
const COLUMNS: usize = 512;

impl Cache {
    /// Drop everything decoded from bytes that are no longer what `id` holds.
    pub fn follow(&mut self, id: u64, stamp: u64) {
        if self.of != Some((id, stamp)) {
            self.of = Some((id, stamp));
            self.zones.clear();
        }
    }

    pub fn get(&self, zone: usize) -> Option<&Result<Decoded, String>> {
        self.zones.get(&zone)
    }

    /// Decode one zone, once. A refusal is remembered like a success: the operator gets
    /// the codec's own reason, and clicking again would only produce it a second time.
    pub fn decode(&mut self, entity: &Entity, zone: usize) {
        if self.zones.contains_key(&zone) {
            return;
        }
        self.zones.insert(zone, decode(entity, zone));
    }
}

fn decode(entity: &Entity, index: usize) -> Result<Decoded, String> {
    let sample = sample(entity).ok_or("this is not a sample instrument")?;
    let layout = sample.layout();
    let zones = sample.zones().map_err(|e| e.to_string())?;
    let zone = zones
        .get(index)
        .ok_or_else(|| format!("there is no zone {}", index + 1))?;
    let audio = codec::decode(zone.stream, zone.at, layout).map_err(|e| e.to_string())?;
    let envelope = envelope(&audio.samples, audio.channels, COLUMNS);
    Ok(Decoded { audio, envelope })
}

/// The min and max of each of `columns` equal slices of the audio, scaled to
/// `-1.0..=1.0`.
///
/// Frames rather than samples, so a stereo zone draws one envelope over both channels
/// instead of two half-width ones. A column with no frames in it — more columns than
/// frames — is flat, which is what a zone shorter than the widget should look like.
pub fn envelope(samples: &[i16], channels: u16, columns: usize) -> Vec<(f32, f32)> {
    let channels = usize::from(channels).max(1);
    let frames = samples.len() / channels;
    if columns == 0 || frames == 0 {
        return Vec::new();
    }
    let scale = |v: i16| f32::from(v) / 32768.0;
    // ⚠️ In 64-bit: a long zone times the column count overflows a 32-bit `usize`, and
    // wasm is a 32-bit target. Every result is at most `frames`, so the cast back is safe.
    let edge = |column: usize| (column as u64 * frames as u64 / columns as u64) as usize;
    (0..columns)
        .map(|column| {
            let from = edge(column);
            let to = edge(column + 1).max(from + 1).min(frames);
            let span = &samples[from * channels..to * channels];
            let low = span.iter().copied().min().unwrap_or(0);
            let high = span.iter().copied().max().unwrap_or(0);
            (scale(low), scale(high))
        })
        .collect()
}

// ---- what the editor keeps between frames -------------------------------------------

/// Which zone is open, what was last struck, and the map the paints are measured
/// against. Nothing here is an edit: an edit is on the working copy the moment it is
/// made.
///
/// ⚠️ Reset when the document changes, which the mock-up's tab strip does too: a row
/// index belongs to the instrument it was opened on.
#[derive(Default)]
pub struct State {
    selected: Option<usize>,
    open: BTreeSet<usize>,
    /// A row to bring up under the pinned map, once the body draws it.
    reveal: Option<usize>,
    /// Whether the 128-key table is unfolded.
    table: bool,
    audition: Option<keys::Audition>,
    /// What the struck key did, so the sentence outlives the click that made it.
    answer: Option<Answer>,
    /// The saved bytes' own keyboard map, read once, so a painted key can be told from
    /// a stored one. `None` inside is a body that carries no map.
    baseline: Option<(u64, Option<KeyTable>)>,
}

/// What a struck key found.
struct Answer {
    /// The zone that answered it, where one did.
    zone: Option<usize>,
    words: String,
    sounded: bool,
}

impl State {
    /// Select a zone, and open its row.
    ///
    /// Clicking the row that is already open and selected closes it; picking from the
    /// map always opens, and asks for the row to be brought into view.
    fn pick(&mut self, zone: usize, reveal: bool) {
        let close = !reveal && self.selected == Some(zone) && self.open.contains(&zone);
        self.selected = Some(zone);
        match close {
            true => self.open.remove(&zone),
            false => self.open.insert(zone),
        };
        if reveal {
            self.reveal = Some(zone);
        }
    }

    /// The zone a live audition is sounding.
    fn lit(&self) -> Option<usize> {
        self.answer.as_ref().and_then(|answer| answer.zone)
    }
}

/// The row the editor has selected — for a widget whose own order is not the row order.
pub fn selected(state: &State) -> Option<usize> {
    state.selected
}

/// Select a row and bring it up under the map, which is what a pick outside the row
/// list means.
pub fn pick_row(state: &mut State, row: usize) {
    state.pick(row, true);
}

// ---- the key map --------------------------------------------------------------------

/// One zone as the key map draws it, whichever format states it.
pub struct MapZone {
    pub low: u8,
    pub top: u8,
    pub root: u8,
    pub name: String,
    /// The velocity window it answers, where the format states one.
    pub velocity: Option<(u8, u8)>,
}

/// Whether a struck key can be heard here.
///
/// A project's audio is not decoded: it is built into an instrument first, and the codec
/// that would do it is not understood — so the map says what answers the key and that
/// nothing will come out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sounds {
    Now,
    NotUntilBuilt,
}

/// What the key map was asked for this frame.
pub enum MapAct {
    /// A handle moved: every zone's `(low, top)` after the clamp, in the order given.
    Bounds(Vec<(u8, u8)>),
    /// A key was struck and a zone answers it, `semitones` from its root.
    Struck { zone: usize, semitones: i16 },
}

/// The keyboard an instrument's zones are laid out over.
///
/// The format states no limit — a note is a byte — so this is the six octaves the
/// editor's own map covers, widened where a zone reaches past it.
const NSMP_SPAN: keys::Span = keys::Span { low: 24, high: 96 };

/// The stretch of keyboard the map must show: the default, and any zone outside it.
fn span(zones: &[MapZone], default: keys::Span) -> keys::Span {
    zones.iter().fold(default, |span, zone| keys::Span {
        low: span.low.min(zone.low),
        high: span.high.max(zone.top),
    })
}

/// The pinned key map: the zone bands, the keyboard under them, and what the last
/// struck key did.
///
/// Both instrument kinds draw this one map. `edges` is what the format lets a pointer
/// move — [`keys::Edges::TopOnly`] where a zone's low is derived from the zone below.
pub fn key_map(
    ui: &mut egui::Ui,
    state: &mut State,
    zones: &[MapZone],
    default: keys::Span,
    edges: keys::Edges,
    sounds: Sounds,
) -> Option<MapAct> {
    let now = ui.input(|input| input.time);
    if state.audition.as_ref().is_some_and(|held| !held.live(now)) {
        state.audition = None;
        state.answer = None;
    }
    let span = span(zones, default);
    let bounds: Vec<(u8, u8)> = zones.iter().map(|zone| (zone.low, zone.top)).collect();
    let silent = keys::gaps(&bounds, span);
    let visuals = ui.visuals().clone();
    let (cover, ink) = match silent.len() {
        0 => ("every key answered".to_string(), app::good(&visuals)),
        1 => ("1 silent range".to_string(), app::warn(&visuals)),
        n => (format!("{n} silent ranges"), app::warn(&visuals)),
    };
    let reading = format!(
        "{cover} · {}–{}",
        note::name(span.low),
        note::name(span.high)
    );
    controls::heading(
        ui,
        "Key map",
        match edges {
            keys::Edges::Fixed => "these zones cannot be written; click a key to hear one",
            keys::Edges::TopOnly => {
                "v2 derives each low from the zone below — drag a top; click a key to hear it"
            }
            keys::Edges::Both => {
                "drag either edge; pull zones apart to leave keys silent, never to \
                 overlap; click a key to hear it"
            }
        },
        Some((&reading, ink)),
    );

    let mut act = None;
    let lane: Vec<keys::Band> = zones
        .iter()
        .map(|zone| keys::Band {
            low: zone.low,
            top: zone.top,
            name: zone.name.clone(),
            range_text: format!("{}–{}", note::name(zone.low), note::name(zone.top)),
            hint: format!(
                "{} · root {} · answers {}–{}",
                zone.name,
                note::name(zone.root),
                note::name(zone.low),
                note::name(zone.top)
            ),
        })
        .collect();
    match keys::bands(ui, span, &lane, state.selected, state.lit(), edges) {
        Some(keys::BandAct::Pick(zone)) => state.pick(zone, true),
        Some(keys::BandAct::Drag { bounds, .. }) => act = Some(MapAct::Bounds(bounds)),
        None => {}
    }

    let marks: Vec<keys::Mark> = zones
        .iter()
        .map(|zone| keys::Mark {
            note: zone.root,
            label: Some(note::name(zone.root)),
        })
        .collect();
    let struck = keys::keyboard(
        ui,
        span,
        state.audition.as_ref().map(|held| held.note),
        &marks,
    );
    if let Some(note) = struck {
        state.audition = Some(keys::Audition::new(note, now));
        let answer = answered(zones, note, sounds);
        if answer.sounded {
            if let Some(zone) = answer.zone {
                act = Some(MapAct::Struck {
                    zone,
                    semitones: i16::from(note) - i16::from(zones[zone].root),
                });
            }
        }
        state.answer = Some(answer);
    }
    if let Some(held) = &state.audition {
        let left = keys::Audition::HOLD - (now - held.started);
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(left.max(0.0)));
    }
    if let Some(answer) = &state.answer {
        status(ui, &answer.words, answer.sounded);
    }
    ui.add_space(8.0);
    act
}

/// What striking `note` at [`keys::AUDITION_VELOCITY`] does, and the sentence that says
/// so.
///
/// A key outside every zone is silent, and so is one inside a zone whose velocity
/// window the struck velocity is outside.
fn answered(zones: &[MapZone], note: u8, sounds: Sounds) -> Answer {
    let velocity = keys::AUDITION_VELOCITY;
    let Some(index) = zones
        .iter()
        .position(|zone| note >= zone.low && note <= zone.top)
    else {
        return Answer {
            zone: None,
            words: format!("{} — no zone answers this key; silence.", note::name(note)),
            sounded: false,
        };
    };
    let zone = &zones[index];
    let found = format!(
        "{} at vel {velocity} → {} · root {} · {}",
        note::name(note),
        zone.name,
        note::name(zone.root),
        keys::shifted(note, zone.root)
    );
    match (zone.velocity, sounds) {
        (Some((low, high)), _) if !(low..=high).contains(&velocity) => Answer {
            zone: Some(index),
            words: format!("{found} — outside its velocity window {low}–{high}; silence"),
            sounded: false,
        },
        (_, Sounds::NotUntilBuilt) => Answer {
            zone: Some(index),
            words: format!("{found} — a project is built into an instrument before it plays"),
            sounded: false,
        },
        (_, Sounds::Now) => Answer {
            zone: Some(index),
            words: found,
            sounded: true,
        },
    }
}

/// The one sentence under the keyboard: what the struck key did.
fn status(ui: &mut egui::Ui, words: &str, sounded: bool) {
    const TEXT: f32 = 11.0;
    const GLYPH: f32 = 12.0;
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        ui.spacing_mut().item_spacing.x = 7.0;
        let (glyph, ink) = match sounded {
            true => (Glyph::AudioLines, app::good(ui.visuals())),
            false => (Glyph::CircleAlert, app::warn(ui.visuals())),
        };
        icon(ui, glyph, GLYPH, ink);
        ui.label(
            egui::RichText::new(words)
                .size(TEXT)
                .color(ui.visuals().weak_text_color()),
        );
    });
}

/// The map over a sample instrument. The sets are the bands a drag moved.
pub fn map(
    ui: &mut egui::Ui,
    state: &mut State,
    snapshot: &Snapshot,
    sets: &mut Sets,
) -> Option<Ask> {
    let zones = map_zones(snapshot);
    let stated = snapshot.zones.iter().all(|zone| zone.low_note.is_some());
    let edges = match (snapshot.zones_editable, stated) {
        (false, _) => keys::Edges::Fixed,
        (true, true) => keys::Edges::Both,
        (true, false) => keys::Edges::TopOnly,
    };
    match key_map(ui, state, &zones, NSMP_SPAN, edges, Sounds::Now)? {
        MapAct::Struck { zone, semitones } => Some(Ask::Strike { zone, semitones }),
        MapAct::Bounds(bounds) => {
            if snapshot.zones_editable {
                sets.extend(moved(&snapshot.zones, &bounds));
            }
            None
        }
    }
}

/// What a moved band writes: the ends that changed, and only the ones the record
/// states — a v2 low is derived from the zone below and follows the top that moved it.
fn moved(zones: &[Zone], bounds: &[(u8, u8)]) -> Sets {
    let mut sets = Sets::new();
    for (index, (low, top)) in bounds.iter().enumerate() {
        let Some(zone) = zones.get(index) else {
            continue;
        };
        let n = index + 1;
        if *top != zone.top_note {
            sets.push((format!("zone{n}.top_note"), note::name(*top)));
        }
        if zone.low_note.is_some_and(|held| held != *low) {
            sets.push((format!("zone{n}.low_note"), note::name(*low)));
        }
    }
    sets
}

/// The instrument's zones as the map states them, with each v2 low derived.
fn map_zones(snapshot: &Snapshot) -> Vec<MapZone> {
    snapshot
        .zones
        .iter()
        .enumerate()
        .map(|(index, zone)| MapZone {
            low: bottom(&snapshot.zones, index).unwrap_or(NSMP_SPAN.low),
            top: zone.top_note,
            root: zone.root_key,
            name: format!("Zone {}", index + 1),
            velocity: zone.velocity,
        })
        .collect()
}

// ---- the rows -----------------------------------------------------------------------

/// One row of the zone list, in the words it prints.
pub struct RowSpec {
    pub name: String,
    pub answers: String,
    /// The middle column: what this zone is made of.
    pub facts: String,
    pub size: String,
    pub hint: String,
}

/// The page's own side margin, which every row and heading keeps.
const PAD: f32 = 12.0;
const HEAD_H: f32 = 20.0;
const ROW_H: f32 = 26.0;
const GAP: f32 = 10.0;
const MARK: f32 = 6.0;
/// The first column, the size column, and the chevron's own.
const NAME_W: f32 = 56.0;
const SIZE_W: f32 = 74.0;
const CHEVRON_W: f32 = 20.0;
/// How far an open row's body is indented, measured from the page's edge.
const INDENT: f32 = 68.0;
/// Which of the five columns holds the size, which is the one set right to left.
const SIZE_COLUMN: usize = 3;
const HEAD_TEXT: f32 = 9.0;
const NAME_TEXT: f32 = 11.5;
const ROW_MONO: f32 = 11.0;
const FACTS_TEXT: f32 = 11.0;
const SIZE_TEXT: f32 = 10.5;
const CHEVRON: f32 = 12.0;

/// The five columns of the row grid: each one's left edge and width.
fn columns(rect: egui::Rect) -> [(f32, f32); 5] {
    let fixed = NAME_W + SIZE_W + CHEVRON_W + GAP * 4.0 + PAD * 2.0;
    let free = (rect.width() - fixed).max(0.0);
    let answers = free * 1.1 / 2.6;
    let facts = free - answers;
    let mut left = rect.left() + PAD;
    let mut out = [(0.0, 0.0); 5];
    for (cell, width) in out
        .iter_mut()
        .zip([NAME_W, answers, facts, SIZE_W, CHEVRON_W])
    {
        *cell = (left, width);
        left += width + GAP;
    }
    out
}

/// The zone list: the column heads, one row per zone, and the body of each open row
/// drawn by `open`.
///
/// `heads` names the first and third columns, which are the two that differ between an
/// instrument and the project it was built from.
pub fn rows(
    ui: &mut egui::Ui,
    state: &mut State,
    heads: (&str, &str),
    specs: &[RowSpec],
    mut open: impl FnMut(&mut egui::Ui, usize),
) {
    let visuals = ui.visuals().clone();
    let hairline = egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color);
    let (head, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEAD_H),
        egui::Sense::hover(),
    );
    {
        let painter = ui.painter();
        painter.hline(head.x_range(), head.top() + 0.5, hairline);
        painter.hline(head.x_range(), head.bottom() - 0.5, hairline);
        let heads = [heads.0, "Answers", heads.1, "Size", ""];
        for (column, ((left, width), text)) in columns(head).into_iter().zip(heads).enumerate() {
            if text.is_empty() {
                continue;
            }
            let galley = painter.layout_no_wrap(
                text.to_uppercase(),
                egui::FontId::proportional(HEAD_TEXT),
                app::caption(&visuals),
            );
            // The size column reads right to left, so its head stands over its figures.
            let left = match column == SIZE_COLUMN {
                true => left + width - galley.size().x,
                false => left,
            };
            painter.galley(
                egui::pos2(left, head.center().y - galley.size().y / 2.0),
                galley,
                app::caption(&visuals),
            );
        }
    }

    let lit = state.lit();
    for (index, spec) in specs.iter().enumerate() {
        let picked = state.selected == Some(index);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_H),
            egui::Sense::click(),
        );
        if state.reveal == Some(index) {
            state.reveal = None;
            response.scroll_to_me(Some(egui::Align::TOP));
        }
        if response.clicked() {
            state.pick(index, false);
        }
        let painter = ui.painter();
        if picked {
            painter.rect_filled(rect, 0.0, visuals.selection.bg_fill);
        } else if response.hovered() {
            painter.rect_filled(rect, 0.0, visuals.widgets.hovered.weak_bg_fill);
        }
        painter.hline(rect.x_range(), rect.bottom() - 0.5, hairline);
        // ⚠️ Every cell of a selected row takes the selection's own ink: the track
        // colour behind it is not readable text.
        let ink = match picked {
            true => visuals.selection.stroke.color,
            false => visuals.weak_text_color(),
        };
        let cells = columns(rect);
        let dot = match (picked, lit == Some(index)) {
            (_, true) => app::good(&visuals),
            (true, false) => app::accent(&visuals),
            (false, false) => app::caption(&visuals),
        };
        painter.circle_filled(
            egui::pos2(cells[0].0 + MARK / 2.0, rect.center().y),
            MARK / 2.0,
            dot,
        );
        let written = [
            (
                spec.name.as_str(),
                egui::FontId::new(NAME_TEXT, app::bold()),
                cells[0].0 + MARK + 6.0,
                cells[0].1 - MARK - 6.0,
                false,
            ),
            (
                spec.answers.as_str(),
                egui::FontId::monospace(ROW_MONO),
                cells[1].0,
                cells[1].1,
                false,
            ),
            (
                spec.facts.as_str(),
                egui::FontId::proportional(FACTS_TEXT),
                cells[2].0,
                cells[2].1,
                false,
            ),
            (
                spec.size.as_str(),
                egui::FontId::monospace(SIZE_TEXT),
                cells[3].0,
                cells[3].1,
                true,
            ),
        ];
        for (text, font, left, width, right) in written {
            let mut job = egui::text::LayoutJob::default();
            job.append(text, 0.0, egui::TextFormat::simple(font, ink));
            job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
            let galley = painter.layout_job(job);
            let at = match right {
                true => left + width - galley.size().x,
                false => left,
            };
            painter.galley(
                egui::pos2(at, rect.center().y - galley.size().y / 2.0),
                galley,
                ink,
            );
        }
        let glyph = match state.open.contains(&index) && picked {
            true => Glyph::ChevronDown,
            false => Glyph::ChevronRight,
        };
        crate::icon::painted(
            ui,
            glyph,
            egui::Rect::from_center_size(
                egui::pos2(cells[4].0 + cells[4].1 - CHEVRON / 2.0, rect.center().y),
                egui::Vec2::splat(CHEVRON),
            ),
            ink,
        );
        if !spec.hint.is_empty() {
            response.on_hover_text(&spec.hint);
        }

        if state.open.contains(&index) && picked {
            let body = egui::Frame::new()
                .fill(visuals.window_fill)
                .inner_margin(egui::Margin {
                    left: INDENT as i8,
                    right: PAD as i8,
                    top: 10,
                    bottom: 12,
                });
            body.show(ui, |ui| {
                ui.set_width(ui.available_width());
                open(ui, index);
            });
            ui.painter()
                .hline(rect.x_range(), ui.min_rect().bottom() - 0.5, hairline);
        }
    }
}

// ---- the cells of an open row -------------------------------------------------------

const LABEL_TEXT: f32 = 9.5;
const VALUE_TEXT: f32 = 11.5;
const ACTION_H: f32 = 20.0;
const ACTION_TEXT: f32 = 11.0;
const ACTION_GLYPH: f32 = 12.0;
const RADIUS: f32 = 2.0;

/// A run of cells across an open row, wrapping where the window is narrow.
pub fn strip(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    let row = egui::Layout::left_to_right(egui::Align::TOP).with_main_wrap(true);
    ui.with_layout(row, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(20.0, 10.0);
        body(ui);
    });
}

/// One cell: a MICRO-caps label with what it names under it.
pub fn cell(ui: &mut egui::Ui, label: &str, width: f32, body: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui(egui::vec2(width, 0.0), |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(
                egui::RichText::new(label.to_uppercase())
                    .size(LABEL_TEXT)
                    .color(app::caption(ui.visuals())),
            );
            body(ui);
        });
    });
}

/// A cell whose value the file states and nothing here writes.
///
/// It asks for the room its own words need: a read cell has no control to size it by.
pub fn read_cell(ui: &mut egui::Ui, label: &str, value: &str, note: &str) {
    let laid = |text: &str, font: egui::FontId| {
        ui.fonts(|fonts| {
            fonts
                .layout_no_wrap(text.to_string(), font, egui::Color32::PLACEHOLDER)
                .size()
                .x
        })
    };
    let width = laid(value, egui::FontId::monospace(VALUE_TEXT)) + 20.0;
    let width = width
        .max(laid(
            &label.to_uppercase(),
            egui::FontId::proportional(LABEL_TEXT),
        ))
        .max(laid(note, egui::FontId::proportional(10.0)));
    cell(ui, label, width, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            icon(ui, Glyph::Eye, 11.0, app::caption(ui.visuals()));
            ui.label(
                egui::RichText::new(value)
                    .font(egui::FontId::monospace(VALUE_TEXT))
                    .color(ui.visuals().weak_text_color()),
            );
        });
        if !note.is_empty() {
            ui.label(
                egui::RichText::new(note)
                    .size(10.0)
                    .color(app::caption(ui.visuals())),
            );
        }
    });
}

/// One outlined action of an open row. `accent` is whether its glyph is the loud one.
pub fn action(ui: &mut egui::Ui, label: &str, glyph: Glyph, accent: bool) -> bool {
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let ink = visuals.weak_text_color();
    let word = painter.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(ACTION_TEXT),
        ink,
    );
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ACTION_GLYPH + word.size().x + 21.0, ACTION_H),
        egui::Sense::click(),
    );
    if response.hovered() {
        painter.rect_filled(rect, RADIUS, visuals.widgets.hovered.weak_bg_fill);
    }
    painter.rect_stroke(
        rect,
        RADIUS,
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    crate::icon::painted(
        ui,
        glyph,
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 8.0 + ACTION_GLYPH / 2.0, rect.center().y),
            egui::Vec2::splat(ACTION_GLYPH),
        ),
        match accent {
            true => app::accent(&visuals),
            false => app::caption(&visuals),
        },
    );
    painter.galley(
        egui::pos2(
            rect.left() + 8.0 + ACTION_GLYPH + 5.0,
            rect.center().y - word.size().y / 2.0,
        ),
        word,
        ink,
    );
    response.clicked()
}

// ---- the Edit face ------------------------------------------------------------------

/// What the document knows about one zone's audio while it draws the zone.
pub struct Sound<'a> {
    pub decoded: Option<&'a Result<Decoded, String>>,
    pub playing: bool,
}

/// The zones, the per-key map, and the preset the loader installs.
///
/// The key map is pinned above this by the document; the name is the header's.
pub fn ui(
    ui: &mut egui::Ui,
    state: &mut State,
    snapshot: &Snapshot,
    sounds: &[Sound],
    sets: &mut Sets,
) -> Option<Ask> {
    let mut ask = None;
    if !snapshot.zones.is_empty() && !snapshot.zones_editable {
        ui.horizontal_wrapped(|ui| {
            ui.add_space(PAD);
            ui.label(
                egui::RichText::new(
                    "This instrument's keyboard map cannot be read, so its zones are \
                     shown rather than changed. The name is still yours to set.",
                )
                .size(FACTS_TEXT)
                .color(app::caption(ui.visuals())),
            );
        });
    }

    velocity(ui, state, snapshot);

    let quiet = app::caption(ui.visuals());
    controls::heading(
        ui,
        "Zones",
        "click a row for its fields",
        Some((&format!("{} zones", snapshot.zones.len()), quiet)),
    );
    let specs: Vec<RowSpec> = snapshot
        .zones
        .iter()
        .enumerate()
        .map(|(index, zone)| RowSpec {
            name: format!("Zone {}", index + 1),
            answers: range(&snapshot.zones, index),
            facts: facts_of(zone, sounds.get(index)),
            size: room::measure(zone.bytes as u64),
            hint: format!("root {}", note::name(zone.root_key)),
        })
        .collect();
    rows(ui, state, ("Zone", "Stroke"), &specs, |ui, index| {
        let zone = &snapshot.zones[index];
        let n = index + 1;
        strip(ui, |ui| {
            ui.add_enabled_ui(snapshot.zones_editable, |ui| {
                cell(ui, "Root key", 72.0, |ui| {
                    if let Some(note) = note_picker(ui, ("root", n), zone.root_key) {
                        sets.push((format!("zone{n}.root_key"), note::name(note)));
                    }
                });
                cell(ui, "Top note", 72.0, |ui| {
                    if let Some(note) = note_picker(ui, ("top", n), zone.top_note) {
                        sets.push((format!("zone{n}.top_note"), note::name(note)));
                    }
                });
            });
            match zone.low_note {
                Some(low) => {
                    ui.add_enabled_ui(snapshot.zones_editable, |ui| {
                        cell(ui, "Low note", 72.0, |ui| {
                            if let Some(note) = note_picker(ui, ("low", n), low) {
                                sets.push((format!("zone{n}.low_note"), note::name(note)));
                            }
                        });
                    });
                }
                None => read_cell(
                    ui,
                    "Low note",
                    &note::name(bottom(&snapshot.zones, index).unwrap_or(NSMP_SPAN.low)),
                    "derived from the zone below",
                ),
            }
            if let Some((low, high)) = zone.velocity {
                read_cell(
                    ui,
                    "Velocity window",
                    &format!("{low}–{high}"),
                    "stated by the record",
                );
            }
            if let Some(gain) = zone.gain {
                read_cell(
                    ui,
                    "Gain",
                    &decibels(gain_db(gain, zone::GAIN_UNITY)),
                    "the zone record's own",
                );
            }
        });
        ui.add_space(10.0);
        if let Some(sound) = sounds.get(index) {
            if let Some(asked) = zone_audio(ui, index, sound) {
                ask = Some(asked);
            }
        }
    });

    if let Some(table) = &snapshot.key_table {
        per_key(ui, state, snapshot, table, sets);
    }
    if !snapshot.sound.is_empty() {
        controls::heading(
            ui,
            "Sound parameters",
            "the loader's preset for this category, read only",
            None,
        );
        ui.horizontal_wrapped(|ui| {
            ui.add_space(PAD);
            strip(ui, |ui| {
                for (label, value) in &snapshot.sound {
                    read_cell(ui, label, value, "");
                }
            });
        });
        ui.add_space(8.0);
    }
    ask
}

/// The key × velocity field, on the generations whose records state a window.
///
/// Read-only: `nord-format` has no setter for a wide zone's window, and every shipped
/// instrument answers the whole of it. Clicking a block still opens its row.
fn velocity(ui: &mut egui::Ui, state: &mut State, snapshot: &Snapshot) {
    let stated: Vec<(usize, &Zone)> = snapshot
        .zones
        .iter()
        .enumerate()
        .filter(|(_, zone)| zone.velocity.is_some())
        .collect();
    if stated.is_empty() {
        return;
    }
    let blocks: Vec<keys::VelBlock> = stated
        .iter()
        .map(|(row, zone)| {
            let window = zone
                .velocity
                .unwrap_or((keys::VELOCITY_LOW, keys::VELOCITY_HIGH));
            keys::VelBlock {
                low: bottom(&snapshot.zones, *row).unwrap_or(NSMP_SPAN.low),
                top: zone.top_note,
                window,
                name: format!("Zone {}", row + 1),
                hint: format!(
                    "Zone {} answers at velocity {}–{} · the record states it and nothing \
                     here writes it",
                    row + 1,
                    window.0,
                    window.1
                ),
            }
        })
        .collect();
    let visuals = ui.visuals().clone();
    let holes = keys::velocity_holes(&blocks);
    let (cover, ink) = match holes.len() {
        0 => ("fully covered".to_string(), app::good(&visuals)),
        1 => ("1 hole".to_string(), app::warn(&visuals)),
        n => (format!("{n} holes"), app::warn(&visuals)),
    };
    controls::heading(
        ui,
        "Velocity",
        "every stroke answers the full window",
        Some((&cover, ink)),
    );
    let span = span(&map_zones(snapshot), NSMP_SPAN);
    let acted = ui
        .horizontal(|ui| {
            ui.add_space(PAD);
            let room = (ui.available_width() - PAD).max(64.0);
            ui.allocate_ui(egui::vec2(room, 0.0), |ui| {
                let picked = stated
                    .iter()
                    .position(|(row, _)| Some(*row) == state.selected);
                keys::velocity(ui, span, &blocks, picked, keys::Handles::Fixed)
            })
            .inner
        })
        .inner;
    ui.add_space(8.0);
    if let Some(keys::VelocityAct::Pick(block)) = acted {
        state.pick(stated[block].0, true);
    }
}

/// The middle column of a zone's row: what the record states, and what a decode found.
fn facts_of(zone: &Zone, sound: Option<&Sound>) -> String {
    let mut parts = Vec::new();
    if let Some(gain) = zone.gain {
        parts.push(decibels(gain_db(gain, zone::GAIN_UNITY)));
    }
    if let Some((low, high)) = zone.velocity {
        parts.push(format!("vel {low}–{high}"));
    }
    if let Some(Ok(decoded)) = sound.and_then(|sound| sound.decoded) {
        parts.push(format!("{:.3} s", decoded.audio.seconds()));
        parts.push(match decoded.audio.channels {
            1 => "mono".to_string(),
            2 => "stereo".to_string(),
            n => format!("{n} channels"),
        });
    }
    parts.join(" · ")
}

/// A gain reading, with the silence a zero field means spelled out rather than as an
/// infinity.
fn decibels(db: f64) -> String {
    match db.is_finite() {
        true => format!("{db:+.1} dB"),
        false => "silent".to_string(),
    }
}

/// The actions of an open zone, and the envelope once it is decoded.
fn zone_audio(ui: &mut egui::Ui, index: usize, sound: &Sound) -> Option<Ask> {
    let mut ask = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        match sound.decoded {
            None => {
                if action(ui, "Show audio", Glyph::AudioLines, true) {
                    ask = Some(Ask::Decode(index));
                }
            }
            Some(Err(why)) => {
                ui.label(
                    egui::RichText::new(format!("not decoded: {why}"))
                        .size(FACTS_TEXT)
                        .color(crate::app::bad(ui.visuals())),
                );
            }
            Some(Ok(_)) => {
                let label = match sound.playing {
                    true => "Stop",
                    false => "Play",
                };
                let glyph = match sound.playing {
                    true => Glyph::X,
                    false => Glyph::AudioLines,
                };
                if action(ui, label, glyph, true) {
                    ask = Some(Ask::Play(index));
                }
                if action(ui, "Save WAV…", Glyph::Waves, false) {
                    ask = Some(Ask::Save(index));
                }
            }
        }
    });
    if let Some(Ok(decoded)) = sound.decoded {
        ui.add_space(8.0);
        waveform(ui, &decoded.envelope, sound.playing);
    }
    ask
}

// ---- the per-key lanes --------------------------------------------------------------

const LANE_LABEL_W: f32 = 80.0;
const LANE_AXIS_W: f32 = 34.0;
const LANE_SUMMARY_W: f32 = 150.0;
const LANE_H: f32 = 38.0;
const AXIS_TEXT: f32 = 9.0;
const SUMMARY_TEXT: f32 = 10.0;
/// What a full-deflection per-key gain and detune read as.
const GAIN_FULL: f32 = 3.0;
const DETUNE_FULL: i32 = 25;

/// The two per-key lanes and the table under them.
///
/// A drag across a lane paints values over the ones the file holds; a key reads as
/// painted when its record differs from the one the asset was last saved with, so the
/// count and the accent bars survive a frame.
fn per_key(
    ui: &mut egui::Ui,
    state: &mut State,
    snapshot: &Snapshot,
    table: &KeyTable,
    sets: &mut Sets,
) {
    let span = span(&map_zones(snapshot), NSMP_SPAN);
    let quiet = app::caption(ui.visuals());
    controls::heading(
        ui,
        "Per key",
        "drag across a lane to draw values; the table below edits one key at a time",
        Some((
            &format!(
                "instrument {}",
                decibels(gain_db(table.instrument.gain(), keymap::GAIN_UNITY))
            ),
            quiet,
        )),
    );
    let baseline = state.baseline.as_ref().and_then(|(_, held)| held.as_ref());
    for (label, scale) in [
        ("Gain", keys::Scale::Db(GAIN_FULL)),
        ("Detune", keys::Scale::Cents(DETUNE_FULL)),
    ] {
        let field = match label {
            "Gain" => "gain",
            _ => "detune",
        };
        let values: Vec<f32> = (span.low..=span.high)
            .map(|note| held(table, note, field))
            .collect();
        let painted: Vec<bool> = (span.low..=span.high)
            .map(|note| match baseline {
                Some(saved) => table.key(note).ok() != saved.key(note).ok(),
                None => false,
            })
            .collect();
        let edited = painted.iter().filter(|edited| **edited).count();
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.allocate_ui(egui::vec2(LANE_LABEL_W, LANE_H), |ui| {
                ui.label(
                    egui::RichText::new(label)
                        .size(FACTS_TEXT)
                        .color(ui.visuals().weak_text_color()),
                );
            });
            axis(ui, scale);
            let room = (ui.available_width() - LANE_SUMMARY_W - 8.0).max(64.0);
            let drawn = ui
                .allocate_ui(egui::vec2(room, LANE_H), |ui| {
                    ui.push_id(field, |ui| keys::lane(ui, span, &values, &painted, scale))
                        .inner
                })
                .inner;
            for (note, value) in drawn {
                sets.push((format!("key{note}.{field}"), scale.format(value)));
            }
            let summary = format!(
                "{} keys · ±{}{}",
                span.keys(),
                match scale {
                    keys::Scale::Db(full) => format!("{full:.1} dB"),
                    keys::Scale::Cents(full) => format!("{full} c"),
                },
                match edited {
                    0 => String::new(),
                    n => format!(" · {n} edited"),
                }
            );
            let ink = match edited {
                0 => app::caption(ui.visuals()),
                _ => app::warn(ui.visuals()),
            };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(summary)
                        .font(egui::FontId::monospace(SUMMARY_TEXT))
                        .color(ink),
                );
            });
        });
        ui.add_space(8.0);
    }
    key_table(ui, state, table);
    ui.add_space(8.0);
}

/// One key's stored value on the lane's own scale, clamped to what it can draw.
fn held(table: &KeyTable, note: u8, field: &str) -> f32 {
    let Ok(level) = table.key(note) else {
        return 0.0;
    };
    let value = match field {
        "gain" => gain_db(level.gain(), keymap::GAIN_UNITY) / f64::from(GAIN_FULL),
        _ => detune_cents(level.detune()) / f64::from(DETUNE_FULL),
    };
    match value.is_finite() {
        true => value.clamp(-1.0, 1.0) as f32,
        false => -1.0,
    }
}

/// The lane's own axis column: the two ends of its scale, and the zero between them.
fn axis(ui: &mut egui::Ui, scale: keys::Scale) {
    let (top, bottom) = scale.axis_labels();
    ui.allocate_ui(egui::vec2(LANE_AXIS_W, LANE_H), |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            for text in [top, "0".to_string(), bottom] {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .font(egui::FontId::monospace(AXIS_TEXT))
                            .color(app::caption(ui.visuals())),
                    );
                });
            }
        });
    });
}

const TABLE_ROW: f32 = 20.0;
const TABLE_TEXT: f32 = 10.5;
const TABLE_COLUMNS: usize = 4;
const TABLE_KEY_W: f32 = 34.0;

/// Every key's record, four columns across, behind a fold.
fn key_table(ui: &mut egui::Ui, state: &mut State, table: &KeyTable) {
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        ui.spacing_mut().item_spacing.x = 6.0;
        let glyph = match state.table {
            true => Glyph::ChevronDown,
            false => Glyph::ChevronRight,
        };
        let quiet = app::caption(ui.visuals());
        icon(ui, glyph, CHEVRON, quiet);
        let label = match state.table {
            true => "Hide the per-key table",
            false => "Show the 128-key gain and detune table",
        };
        if ui
            .add(
                egui::Label::new(
                    egui::RichText::new(label)
                        .size(FACTS_TEXT)
                        .color(ui.visuals().weak_text_color()),
                )
                .sense(egui::Sense::click()),
            )
            .clicked()
        {
            state.table = !state.table;
        }
    });
    if !state.table {
        return;
    }
    let per_column = keymap::KEYS.div_ceil(TABLE_COLUMNS);
    let visuals = ui.visuals().clone();
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), per_column as f32 * TABLE_ROW),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    let inner = rect.shrink2(egui::vec2(PAD, 0.0));
    let column_w = inner.width() / TABLE_COLUMNS as f32;
    for note in 0..keymap::KEYS {
        let cell = egui::Rect::from_min_size(
            egui::pos2(
                inner.left() + (note / per_column) as f32 * column_w,
                inner.top() + (note % per_column) as f32 * TABLE_ROW,
            ),
            egui::vec2(column_w - 18.0, TABLE_ROW),
        );
        painter.hline(
            cell.x_range(),
            cell.bottom() - 0.5,
            egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
        );
        let Ok(level) = table.key(note as u8) else {
            continue;
        };
        let loud = |quiet: bool| match quiet {
            true => app::caption(&visuals),
            false => visuals.text_color(),
        };
        let written = [
            (
                note::name(note as u8),
                visuals.weak_text_color(),
                cell.left(),
                false,
            ),
            (
                decibels(gain_db(level.gain(), keymap::GAIN_UNITY)),
                loud(level.gain() == keymap::GAIN_UNITY),
                cell.left() + TABLE_KEY_W + (cell.width() - TABLE_KEY_W) / 2.0,
                true,
            ),
            (
                format!("{:+.0} c", detune_cents(level.detune())),
                loud(level.detune() == 0),
                cell.right(),
                true,
            ),
        ];
        for (text, ink, at, right) in written {
            let galley = painter.layout_no_wrap(text, egui::FontId::monospace(TABLE_TEXT), ink);
            let left = match right {
                true => at - galley.size().x,
                false => at,
            };
            painter.galley(
                egui::pos2(left, cell.center().y - galley.size().y / 2.0),
                galley,
                ink,
            );
        }
    }
}

/// Read the map the asset was last saved with, once per document.
///
/// ⚠️ Paint marks are the difference between what is held and what was saved, so the
/// baseline has to be the saved bytes rather than the working copy: measured against
/// itself, nothing is ever painted.
pub fn follow(state: &mut State, id: u64, saved: &[u8]) {
    if state.baseline.as_ref().is_some_and(|(held, _)| *held == id) {
        return;
    }
    let table = nord_format::from_stream(&mut Cursor::new(saved))
        .ok()
        .as_ref()
        .and_then(sample)
        .and_then(|sample| match sample {
            Sample::V2(body) => body.key_table().ok(),
            Sample::V3(_) => None,
        });
    state.baseline = Some((id, table));
}

// ---- the other two faces ------------------------------------------------------------

/// The identity cell an instrument puts on the header: what the file states about
/// itself that the strip cannot carry.
///
/// Both are read-only because neither has a setter: the `cat` section and the wide
/// chain's second name are shown as the file holds them.
pub fn stated(entity: &Entity) -> Option<Cell> {
    match sample(entity)? {
        Sample::V2(body) => {
            let categories = body.categories();
            match categories.is_empty() {
                true => None,
                false => Some(Cell {
                    label: "Category",
                    body: Body::Read(categories.join(", ")),
                    note: None,
                    hint: "the cat section, as the file holds it",
                }),
            }
        }
        Sample::V3(body) => {
            let sub = body.sub_name().unwrap_or_default();
            match sub.trim().is_empty() {
                true => None,
                false => Some(Cell {
                    label: "Sub name",
                    body: Body::Read(sub),
                    note: None,
                    hint: "the vendor's second name",
                }),
            }
        }
    }
}

/// What the file says about itself.
pub fn metadata(ui: &mut egui::Ui, snapshot: &Snapshot) {
    controls::heading(
        ui,
        "Metadata",
        "what the file says about itself — read here, never written differently",
        None,
    );
    let mut rows = vec![
        Fact {
            key: "Format",
            value: format!("nsmp {}", snapshot.generation),
            note: "a sample instrument",
        },
        Fact {
            key: "Content version",
            value: snapshot.version.to_string(),
            note: "what decides which fields this format has",
        },
        Fact {
            key: "Name in file",
            value: format!(
                "{} B, {} used",
                snapshot.max_name_len,
                snapshot.name.trim_end().len()
            ),
            note: "the whole field is written; the rest is padding",
        },
        Fact {
            key: "Zones",
            value: format!("{} × {} B", snapshot.zones.len(), snapshot.record_len),
            note: "one record per zone, in the map section",
        },
        Fact {
            key: "Channels",
            value: "per stroke".to_string(),
            note: "shown on each zone once it is decoded",
        },
    ];
    if !snapshot.sub_name.trim().is_empty() {
        rows.push(Fact {
            key: "Sub name",
            value: snapshot.sub_name.clone(),
            note: "the vendor's second name",
        });
    }
    if snapshot.key_table.is_some() {
        rows.push(Fact {
            key: "Keyboard map",
            value: format!("{} × {} B", keymap::KEYS, keymap::RECORD_LEN),
            note: "gain and detune per key, ahead of the zone table",
        });
    }
    super::capability::facts(ui, &rows);
}

/// The nineteen capabilities of the instrument editor, as this generation stands in
/// them.
///
/// `Editable` is a field a control on the Edit face writes or an act it performs,
/// `ReadOnly` a field the format states and nothing here writes, `Absent` a field the
/// format does not have at all. The table is checked against the paths [`set`] accepts —
/// see the tests.
pub fn capabilities(generation: &str) -> Vec<Row> {
    let v2 = generation == "v2";
    let row = |name: &'static str, state: Cap, note: &'static str| Row { name, state, note };
    vec![
        row(
            "name",
            Cap::Editable,
            match v2 {
                true => "32 B in the hdr section",
                false => "66 B in the hdr section",
            },
        ),
        row(
            "category / sub",
            Cap::ReadOnly,
            match v2 {
                true => "the cat section, on the identity row",
                false => "the sub name, on the identity row",
            },
        ),
        row(
            "key zones: root / top / low",
            Cap::Editable,
            match v2 {
                true => "the zone table; each low is derived from the zone below",
                false => "the wide zone record states all three",
            },
        ),
        row(
            "velocity layers",
            match v2 {
                true => Cap::Absent,
                false => Cap::ReadOnly,
            },
            match v2 {
                true => "dropped by v2",
                false => "one wide window per zone, and no setter for it",
            },
        ),
        row(
            "per-zone gain / detune",
            match v2 {
                true => Cap::ReadOnly,
                false => Cap::Absent,
            },
            match v2 {
                true => "a u24 gain in the record, and no detune beside it",
                false => "a dB float in the stroke header, which is not decoded",
            },
        ),
        row(
            "per-key table",
            match v2 {
                true => Cap::Editable,
                false => Cap::Absent,
            },
            match v2 {
                true => "128 records of gain and detune",
                false => "no keyboard map the wide layouts expose",
            },
        ),
        row(
            "instrument gain",
            match v2 {
                true => Cap::ReadOnly,
                false => Cap::Absent,
            },
            match v2 {
                true => "the map's own record, read on the Per key heading",
                false => "not in the wide chain's map",
            },
        ),
        row(
            "loop points / crossfade",
            Cap::Absent,
            "baked into the stroke, with no mark this editor reads",
        ),
        row(
            "loop decay / detune",
            Cap::Absent,
            match v2 {
                true => "dropped by v2",
                false => "in the stroke header, which is written back verbatim",
            },
        ),
        row("release samples", Cap::Absent, "a piano library's bank 2"),
        row(
            "pedal resonance samples",
            Cap::Absent,
            "a piano library's bank 1",
        ),
        row(
            "sound parameters",
            Cap::ReadOnly,
            "the sty preset the loader installs for this category",
        ),
        row(
            "stereo / channels",
            Cap::ReadOnly,
            "per stroke, and known once the zone is decoded",
        ),
        row(
            "replace / add a stroke",
            Cap::NeedsEncode,
            match v2 {
                true => "encode a WAV; played on hardware",
                false => "encode a WAV; byte-exact, never played here",
            },
        ),
        row(
            "cut / move / drop strokes",
            Cap::ReadOnly,
            "the zone table is read; nothing here removes a zone",
        ),
        row("decode / audition", Cap::Editable, "click a key, or a zone"),
        row(
            "size trim",
            Cap::ReadOnly,
            "each zone's bytes are read; nothing here drops one",
        ),
        row(
            "write to the instrument",
            Cap::Editable,
            match v2 {
                true => "the header's Queue send, a class 3 write",
                false => "the header's Queue send",
            },
        ),
        row(
            "byte-exact round trip",
            Cap::Verified,
            "every stroke is written back byte for byte",
        ),
    ]
}

/// Where each field the Edit face reads or writes lands in the file.
///
/// Every figure is one of `nord_format`'s own declarations rather than a measurement.
pub fn offsets(snapshot: &Snapshot) -> Vec<Offset> {
    let mut rows = vec![Offset {
        at: format!("map+{}", zone::COUNT_AT),
        holds: "u8".to_string(),
        note: "how many zone records follow",
    }];
    if snapshot.key_table.is_some() {
        rows.insert(
            0,
            Offset {
                at: "map+0".to_string(),
                holds: format!("{} B", keymap::RECORD_LEN),
                note: "the instrument's own gain and detune",
            },
        );
        rows.insert(
            1,
            Offset {
                at: format!("map+{}", keymap::KEY_TABLE_AT),
                holds: format!("{} × {} B", keymap::KEYS, keymap::RECORD_LEN),
                note: "a u24 gain and an s24 detune per MIDI note",
            },
        );
    }
    rows.push(Offset {
        at: format!("map+{}", zone::RECORDS_AT),
        holds: format!("{} × {} B", snapshot.zones.len(), snapshot.record_len),
        note: "root, top note, stroke id, gain, relative strength",
    });
    rows
}

// ---- paint -------------------------------------------------------------------------

/// Height of the drawn envelope.
const WAVE_HEIGHT: f32 = 44.0;

/// Draw an envelope across whatever width is left.
///
/// Painted from the theme's own colours rather than fixed ones: the trough is the panel's
/// extreme fill, the wave is the instrument's red while it is sounding and the body text
/// colour when it is not, so both themes stay legible.
pub fn waveform(ui: &mut egui::Ui, envelope: &[(f32, f32)], playing: bool) {
    let width = ui.available_width().max(64.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, WAVE_HEIGHT), egui::Sense::hover());
    let visuals = ui.visuals();
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    let middle = rect.center().y;
    painter.hline(
        rect.x_range(),
        middle,
        egui::Stroke::new(1.0_f32, crate::app::unlit(visuals)),
    );
    if envelope.is_empty() {
        return;
    }
    let ink = match playing {
        true => crate::app::accent(visuals),
        false => visuals.text_color(),
    };
    // The envelope has a fixed column count and the panel does not, so a column is as
    // wide as its share of the rect — never thinner than the pixel it has to cover.
    let column = (rect.width() / envelope.len() as f32).max(1.0);
    let half = rect.height() / 2.0 - 1.0;
    for (i, (low, high)) in envelope.iter().enumerate() {
        let x = rect.left() + rect.width() * i as f32 / envelope.len() as f32;
        let top = middle - high.clamp(-1.0, 1.0) * half;
        let bottom = middle - low.clamp(-1.0, 1.0) * half;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x, top),
                egui::pos2(x + column, bottom.max(top + 1.0)),
            ),
            0.0,
            ink,
        );
    }
}

/// A MIDI note as a name: `C4` is middle C. Typing a number works too.
pub fn note_picker(ui: &mut egui::Ui, id: (&str, usize), note: u8) -> Option<u8> {
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
    (response.inner.changed() && picked != note).then_some(picked)
}

#[cfg(test)]
mod tests {
    use nord_format::cbin::Cbin;
    use nord_format::formats::nsmp;

    use super::*;

    fn zone(root_key: u8, top_note: u8, low_note: Option<u8>) -> Zone {
        Zone {
            root_key,
            top_note,
            low_note,
            gain: None,
            velocity: None,
            bytes: 0,
        }
    }

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
        let zones = vec![zone(72, 96, None), zone(60, 71, None)];
        // Zone 2 tops out at B4, so zone 1 starts one key above it.
        assert_eq!(range(&zones, 0), "C5 up to C7");
        assert_eq!(range(&zones, 1), "up to B4");

        // A file that states its own bottom is believed rather than derived.
        let stated = vec![zone(60, 71, Some(48))];
        assert_eq!(range(&stated, 0), "C3 up to B4");
    }

    #[test]
    fn an_envelope_reduces_the_audio_to_one_pair_per_column() {
        // Four frames of mono, two columns: each column covers two frames.
        let mono = [i16::MAX, 0, -8192, 8192];
        let pairs = envelope(&mono, 1, 2);
        assert_eq!(pairs.len(), 2);
        assert!((pairs[0].1 - 0.999_97).abs() < 1e-4, "{:?}", pairs[0]);
        assert_eq!(pairs[0].0, 0.0);
        assert_eq!(pairs[1], (-0.25, 0.25));

        // Stereo: one envelope over both channels, not two half-width ones.
        let stereo = [0, i16::MIN, 0, 0];
        assert_eq!(envelope(&stereo, 2, 1), vec![(-1.0, 0.0)]);

        // Fewer frames than columns: every column still gets a pair, and none is empty.
        let short = [1000i16, -1000];
        assert_eq!(envelope(&short, 1, 8).len(), 8);
        // Degenerate asks answer with nothing rather than an empty span or a divide.
        assert!(envelope(&mono, 1, 0).is_empty());
        assert!(envelope(&[], 1, 4).is_empty());
    }

    /// A wide-chain instrument reads its own name, sub-name and generation, and its
    /// zones are editable where the `map` layout does not name every key.
    #[test]
    fn a_later_generation_reads_and_edits() {
        let entity = Entity::Sample(Sample::V3(v3_sample(300)));
        assert!(is_sample(&entity));

        let snapshot = snapshot(&entity).expect("a sample").expect("it reads");
        assert_eq!(snapshot.name, "Bass Clarinet");
        assert_eq!(snapshot.sub_name, "KG  mono");
        assert_eq!(snapshot.generation, "v3");
        assert!(snapshot.zones_editable);
        let zones: Vec<(u8, u8, Option<u8>)> = snapshot
            .zones
            .iter()
            .map(|zone| (zone.root_key, zone.top_note, zone.low_note))
            .collect();
        assert_eq!(zones, [(72, 96, Some(61)), (60, 60, Some(17))]);
        assert_eq!(range(&snapshot.zones, 0), "C#4 up to C7");
        assert!(
            snapshot.key_table.is_none(),
            "the wide layouts expose no keyboard map"
        );
    }

    #[test]
    fn a_v4_instrument_says_so() {
        let entity = Entity::Sample(Sample::V3(v3_sample(400)));
        let snapshot = snapshot(&entity).unwrap().unwrap();
        assert_eq!(snapshot.generation, "v4");
    }

    /// One second of 44.1 kHz mono, and the one-zone v2 instrument the encoder makes
    /// of it — the only instrument this app can build from nothing.
    fn v2_bytes() -> Vec<u8> {
        let samples: Vec<i16> = (0..codec::SOURCE_RATE as usize)
            .map(|i| ((i as f64 / 40.0).sin() * 12_000.0) as i16)
            .collect();
        let options = nsmp::encode::Options::new("Marimba").root_key(60);
        nsmp::encode::instrument(&samples, &options)
            .unwrap()
            .to_bytes()
            .unwrap()
    }

    fn table_of(bytes: &[u8]) -> KeyTable {
        let entity = nord_format::from_stream(&mut Cursor::new(bytes)).unwrap();
        snapshot(&entity).unwrap().unwrap().key_table.unwrap()
    }

    /// Where two sets of bytes differ, as `(start, length)` runs.
    fn runs(was: &[u8], is: &[u8]) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (at, _) in was
            .iter()
            .zip(is)
            .enumerate()
            .filter(|(_, (was, is))| was != is)
        {
            match out.last_mut() {
                Some(run) if run.0 + run.1 == at => run.1 += 1,
                _ => out.push((at, 1)),
            }
        }
        out
    }

    /// The two per-key units are the record's own: unity gain reads as no change at
    /// all, and a semitone of detune is a hundred cents.
    #[test]
    fn a_per_key_reading_is_the_record_in_its_own_unit() {
        assert_eq!(gain_db(keymap::GAIN_UNITY, keymap::GAIN_UNITY), 0.0);
        assert_eq!(gain_units(0.0).unwrap(), keymap::GAIN_UNITY);
        assert_eq!(detune_cents(keymap::DETUNE_PER_SEMITONE), 100.0);
        assert_eq!(detune_units(100.0), keymap::DETUNE_PER_SEMITONE);
        assert_eq!(detune_units(-100.0), -keymap::DETUNE_PER_SEMITONE);

        // Halving the ratio is six decibels down, and the reading goes back the way it
        // came.
        let half = gain_units(-6.020_6).unwrap();
        assert_eq!(half, keymap::GAIN_UNITY / 2);
        assert!(gain_db(half, keymap::GAIN_UNITY) < -6.0);
        // A zero field is silence, not a reading.
        assert!(!gain_db(0, keymap::GAIN_UNITY).is_finite());
        assert_eq!(decibels(gain_db(0, keymap::GAIN_UNITY)), "silent");
        // More gain than the 24-bit field holds is refused rather than wrapped.
        assert!(gain_units(48.0).is_err());
    }

    /// What the lane writes is what the lane reads: the value it paints is spelled in
    /// the unit the setter parses.
    #[test]
    fn a_painted_value_round_trips_through_the_path_it_is_written_on() {
        assert_eq!(keys::Scale::Db(GAIN_FULL).format(0.5), "+1.5 dB");
        assert_eq!(measured("+1.5 dB", "dB").unwrap(), 1.5);
        assert_eq!(measured("-12 c", "c").unwrap(), -12.0);
        assert_eq!(key_path("key60.gain"), Some((60, "gain")));
        assert_eq!(key_path("key127.detune"), Some((127, "detune")));
        assert_eq!(key_path("key128.gain"), None, "not a MIDI note");
        assert_eq!(key_path("zone1.top_note"), None);
        assert!(measured("loud", "dB").is_err());
    }

    /// ⚠️ The keyboard map is one field: a painted key must leave every other key's
    /// record alone, and the key's own other half with it.
    #[test]
    fn a_painted_key_moves_its_own_record_and_nothing_else() {
        let bytes = v2_bytes();
        let before = table_of(&bytes);
        assert_eq!(before.key(60).unwrap(), Level::NEUTRAL);

        let edited = apply(
            &bytes,
            &[
                ("key60.gain".into(), "+1.5 dB".into()),
                ("key61.detune".into(), "-12 c".into()),
            ],
        )
        .unwrap();
        let after = table_of(&edited);

        assert_eq!(after.key(60).unwrap().gain(), gain_units(1.5).unwrap());
        assert_eq!(after.key(60).unwrap().detune(), 0, "the other half stands");
        assert_eq!(after.key(61).unwrap().detune(), detune_units(-12.0));
        assert_eq!(
            after.key(61).unwrap().gain(),
            keymap::GAIN_UNITY,
            "the other half stands"
        );
        assert_eq!(after.instrument, before.instrument);
        assert_eq!(after.adjusted().collect::<Vec<_>>(), [60, 61]);

        // Byte isolation: the container's own checksum word, and one three-byte half of
        // each record — the gain of key 60 and the detune of key 61. Nothing else in
        // the file moved, the other half of each record included.
        assert_eq!(bytes.len(), edited.len());
        let moved = runs(&bytes, &edited);
        assert_eq!(moved.len(), 3, "{moved:?}");
        assert_eq!(moved[0].1, 4, "the container's checksum word: {moved:?}");
        assert_eq!((moved[1].1, moved[2].1), (3, 3), "{moved:?}");
        assert_eq!(
            moved[2].0 - moved[1].0,
            keymap::RECORD_LEN + 3,
            "the next key's other half: {moved:?}"
        );
    }

    /// A key edit and a zone edit in one frame are one apply, and neither is lost.
    #[test]
    fn a_key_and_a_zone_edit_land_together() {
        let bytes = v2_bytes();
        let edited = apply(
            &bytes,
            &[
                ("key60.gain".into(), "+1.5 dB".into()),
                ("zone1.top_note".into(), "C6".into()),
                ("key60.detune".into(), "+50 c".into()),
            ],
        )
        .unwrap();
        let entity = nord_format::from_stream(&mut Cursor::new(&edited)).unwrap();
        let snapshot = snapshot(&entity).unwrap().unwrap();
        assert_eq!(snapshot.zones[0].top_note, 84);
        let key = snapshot.key_table.unwrap().key(60).unwrap();
        assert_eq!(key.gain(), gain_units(1.5).unwrap());
        assert_eq!(key.detune(), detune_units(50.0));
    }

    /// A path that names no field is refused before anything is written.
    #[test]
    fn unknown_key_paths_are_refused() {
        let bytes = v2_bytes();
        for (path, value) in [
            ("key60.loudness", "+1.5 dB"),
            ("key60.gain", "loud"),
            ("key60.gain", "+48.0 dB"),
        ] {
            assert!(
                apply(&bytes, &[(path.into(), value.into())]).is_err(),
                "{path} = {value}"
            );
        }
        // The wide chain has no map to write, and says so rather than writing one.
        let wide = nord_format::to_bytes(&Entity::Sample(Sample::V3(v3_sample(300)))).unwrap();
        assert!(apply(&wide, &[("key60.gain".into(), "+1.5 dB".into())]).is_err());
    }

    /// Every path in [`EDITS`], which is what the capability table's editable rows are
    /// checked against.
    const EDITS: [(&str, Option<&str>); 6] = [
        ("name", Some("name")),
        ("key zones: root / top / low", Some("zone1.top_note")),
        ("per-key table", Some("key60.gain")),
        // The two the Edit face performs as acts rather than field writes.
        ("decode / audition", None),
        ("write to the instrument", None),
        // Named here so the wide generations' table is covered by the same check.
        ("velocity layers", None),
    ];

    /// ⚠️ The capability table is a claim about this editor, not a wish list. Every row
    /// it calls editable has to name something the editor can actually write, or be one
    /// of the acts named in [`EDITS`].
    #[test]
    fn every_editable_capability_names_a_path_the_editor_accepts() {
        let bytes = v2_bytes();
        for generation in ["v2", "v3", "v4"] {
            for row in capabilities(generation) {
                if row.state != Cap::Editable {
                    continue;
                }
                let named = EDITS
                    .iter()
                    .find(|(name, _)| *name == row.name)
                    .unwrap_or_else(|| {
                        panic!("{generation}: {} is editable and unlisted", row.name)
                    });
                let Some(path) = named.1 else { continue };
                if generation != "v2" {
                    continue;
                }
                assert!(
                    apply(&bytes, &[(path.into(), example(path))]).is_ok(),
                    "{}: {path} is not a path the editor accepts",
                    row.name
                );
            }
        }
        // A row the table calls editable on the wide generations but not on v2 is only
        // ever listed as an act: nothing here writes a velocity window.
        assert!(capabilities("v3")
            .iter()
            .all(|row| row.name != "velocity layers" || row.state == Cap::ReadOnly));
    }

    /// A value each example path takes.
    fn example(path: &str) -> String {
        match path {
            "name" => "Vibes".to_string(),
            "zone1.top_note" => "C6".to_string(),
            _ => "+1.5 dB".to_string(),
        }
    }

    /// The identity row carries what the strip cannot: a narrow instrument's category
    /// and a wide one's second name, both read-only.
    #[test]
    fn the_identity_cell_is_what_the_file_states_about_itself() {
        let wide = Entity::Sample(Sample::V3(v3_sample(300)));
        let cell = stated(&wide).expect("a sub name");
        assert_eq!(cell.label, "Sub name");
        match cell.body {
            Body::Read(value) => assert_eq!(value, "KG  mono"),
            Body::Chips(_) => panic!("a sub name is not a chip"),
        }

        let narrow = nord_format::from_stream(&mut Cursor::new(&v2_bytes())).unwrap();
        let cell = stated(&narrow).expect("the encoder writes a cat section");
        assert_eq!(cell.label, "Category");
        match cell.body {
            Body::Read(value) => assert!(!value.is_empty(), "the cat section is read"),
            Body::Chips(_) => panic!("a category is not a chip"),
        }
    }

    /// Every offset the Advanced face prints is one of the format's own declarations.
    #[test]
    fn the_offsets_are_the_formats_own_declarations() {
        let entity = nord_format::from_stream(&mut Cursor::new(&v2_bytes())).unwrap();
        let narrow = snapshot(&entity).unwrap().unwrap();
        let rows = offsets(&narrow);
        let at: Vec<&str> = rows.iter().map(|row| row.at.as_str()).collect();
        assert_eq!(
            at,
            [
                "map+0",
                &format!("map+{}", keymap::KEY_TABLE_AT),
                &format!("map+{}", zone::COUNT_AT),
                &format!("map+{}", zone::RECORDS_AT),
            ]
        );
        assert_eq!(narrow.record_len, 15, "the Library 2 zone record");

        // A body with no keyboard map prints no keyboard map offsets.
        let entity = Entity::Sample(Sample::V3(v3_sample(300)));
        let wide = snapshot(&entity).unwrap().unwrap();
        assert_eq!(offsets(&wide).len(), 2);
    }

    /// Picking from the map opens a row and asks for it; clicking an open, selected row
    /// closes it again. An edit is never what a pick changes.
    #[test]
    fn picking_a_zone_opens_its_row_and_clicking_it_again_closes_it() {
        let mut state = State::default();
        state.pick(1, true);
        assert_eq!(state.selected, Some(1));
        assert!(state.open.contains(&1));
        assert_eq!(state.reveal, Some(1), "the map asked for it to be shown");

        state.reveal = None;
        state.pick(1, false);
        assert!(!state.open.contains(&1), "the same row closes");
        assert_eq!(state.reveal, None);

        // Another row opens rather than toggling the one that was open.
        state.pick(0, false);
        state.pick(1, false);
        assert!(state.open.contains(&1));
    }

    /// What a struck key does: the zone that answers it, or why nothing does.
    #[test]
    fn a_struck_key_says_which_zone_answers_it() {
        let zones = [
            MapZone {
                low: 61,
                top: 96,
                root: 72,
                name: "Zone 1".into(),
                velocity: None,
            },
            MapZone {
                low: 24,
                top: 40,
                root: 48,
                name: "Zone 2".into(),
                velocity: Some((1, 64)),
            },
        ];

        let heard = answered(&zones, 84, Sounds::Now);
        assert_eq!(heard.zone, Some(0));
        assert!(heard.sounded);
        assert_eq!(
            heard.words,
            "C6 at vel 90 → Zone 1 · root C5 · shifted +12 st"
        );

        let silent = answered(&zones, 50, Sounds::Now);
        assert_eq!(silent.zone, None);
        assert!(!silent.sounded);
        assert_eq!(silent.words, "D3 — no zone answers this key; silence.");

        // Inside the zone but outside the window it answers at.
        let quiet = answered(&zones, 24, Sounds::Now);
        assert_eq!(quiet.zone, Some(1));
        assert!(!quiet.sounded);
        assert!(
            quiet
                .words
                .ends_with("outside its velocity window 1–64; silence"),
            "{}",
            quiet.words
        );
    }

    /// The map shows every zone, including one that reaches past the six octaves it
    /// opens on — a band off the end of the span is a band over the wrong key.
    #[test]
    fn the_span_widens_to_hold_every_zone() {
        let inside = [MapZone {
            low: 36,
            top: 84,
            root: 60,
            name: "Zone 1".into(),
            velocity: None,
        }];
        assert_eq!(span(&inside, NSMP_SPAN), NSMP_SPAN);

        let past = [MapZone {
            low: 17,
            top: 108,
            root: 60,
            name: "Zone 1".into(),
            velocity: None,
        }];
        assert_eq!(span(&past, NSMP_SPAN), keys::Span { low: 17, high: 108 });
    }

    // ---- the pinned map ------------------------------------------------------------

    /// A context dressed as the app dresses it: the semibold family a band and a row
    /// are set in is not bound by default, and laying one out without it panics.
    fn dressed() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::fonts());
        ctx.set_visuals(egui::Visuals::dark());
        ctx
    }

    /// One frame of the pinned map over `snapshot`: what it painted, what it wrote, and
    /// what it asked for.
    fn mapped(
        ctx: &egui::Context,
        state: &mut State,
        snapshot: &Snapshot,
        events: Vec<egui::Event>,
    ) -> (Vec<(String, egui::Rect)>, Sets, Option<Ask>) {
        let mut sets = Sets::new();
        let mut ask = None;
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
                ask = map(ui, state, snapshot, &mut sets);
            });
        });
        let mut said = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut said);
        }
        (said, sets, ask)
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

    /// The lowest place a word was painted — the keyboard's own octave labels sit at
    /// the bottom of the key they name, under everything else that spells a note.
    fn lowest(said: &[(String, egui::Rect)], word: &str) -> egui::Rect {
        said.iter()
            .filter(|(text, _)| text == word)
            .map(|(_, at)| *at)
            .reduce(|a, b| match a.center().y > b.center().y {
                true => a,
                false => b,
            })
            .unwrap_or_else(|| panic!("{word} was never painted: {said:?}"))
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

    fn v2_snapshot() -> Snapshot {
        let entity = nord_format::from_stream(&mut Cursor::new(&v2_bytes())).unwrap();
        snapshot(&entity).unwrap().unwrap()
    }

    /// One frame of the body over `snapshot`, with nothing decoded: what it painted,
    /// and what it wrote.
    fn bodied(ctx: &egui::Context, state: &mut State, snapshot: &Snapshot) -> (Vec<String>, Sets) {
        let mut sets = Sets::new();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 900.0),
            )),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            ctx.style_mut(crate::app::metrics);
            egui::CentralPanel::default().show(ctx, |page| {
                ui(page, state, snapshot, &[], &mut sets);
            });
        });
        let mut said = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut said);
        }
        (said.into_iter().map(|(text, _)| text).collect(), sets)
    }

    /// The wide generations state a velocity window per zone, so the field is drawn —
    /// read only, because nothing here writes one. A v2 record holds no window at all,
    /// and gets no section for it.
    #[test]
    fn the_velocity_field_is_drawn_only_where_a_window_is_stated() {
        let ctx = dressed();
        let entity = Entity::Sample(Sample::V3(v3_sample(300)));
        let wide = snapshot(&entity).unwrap().unwrap();
        assert!(wide.zones.iter().all(|zone| zone.velocity.is_some()));
        let (said, sets) = bodied(&ctx, &mut State::default(), &wide);
        assert!(said.iter().any(|text| text == "Velocity"), "{said:?}");
        assert!(
            said.iter()
                .any(|text| text == "every stroke answers the full window"),
            "{said:?}"
        );
        assert!(sets.is_empty(), "a stated window is not an edit");

        let narrow = v2_snapshot();
        assert!(narrow.zones.iter().all(|zone| zone.velocity.is_none()));
        let (said, sets) = bodied(&ctx, &mut State::default(), &narrow);
        assert!(!said.iter().any(|text| text == "Velocity"), "{said:?}");
        assert!(
            said.iter().any(|text| text == "Per key"),
            "the v2 keyboard map is drawn instead: {said:?}"
        );
        assert!(sets.is_empty());
    }

    /// Clicking a key a zone answers sounds it and says so; clicking one past every
    /// zone says why nothing came out. Neither is an edit.
    #[test]
    fn a_click_on_the_keyboard_sounds_the_zone_that_answers_it() {
        let ctx = dressed();
        let snapshot = v2_snapshot();
        assert_eq!(snapshot.zones.len(), 1);
        assert_eq!(
            (snapshot.zones[0].root_key, snapshot.zones[0].top_note),
            (60, 84),
            "the encoder's own one-zone instrument"
        );

        let mut state = State::default();
        let (said, sets, ask) = mapped(&ctx, &mut state, &snapshot, Vec::new());
        assert!(ask.is_none(), "nothing sounds unasked");
        assert!(sets.is_empty());
        assert!(
            said.iter()
                .any(|(text, _)| text == "1 silent range · C1–C7"),
            "the keys above the zone answer nothing: {said:?}"
        );

        let middle_c = lowest(&said, "C4").center();
        let (said, sets, ask) = mapped(&ctx, &mut state, &snapshot, press(middle_c));
        assert_eq!(
            ask,
            Some(Ask::Strike {
                zone: 0,
                semitones: 0
            })
        );
        assert!(sets.is_empty(), "a struck key is never an edit");
        assert!(
            said.iter()
                .any(|(text, _)| text == "C4 at vel 90 → Zone 1 · root C4 · shifted +0 st"),
            "{said:?}"
        );

        // A key past the zone's top: the sentence says silence, and nothing is asked.
        let above = lowest(&said, "C7").center();
        let (said, _, ask) = mapped(&ctx, &mut state, &snapshot, press(above));
        assert_eq!(ask, None);
        assert!(
            said.iter()
                .any(|(text, _)| text == "C7 — no zone answers this key; silence."),
            "{said:?}"
        );
    }

    /// A moved band writes the ends the record states, and nothing that did not move.
    ///
    /// ⚠️ A v2 low is derived from the zone below rather than stored, so a drag that
    /// moves one must not write it: the setter refuses, and the whole apply is lost.
    #[test]
    fn a_moved_band_writes_only_the_ends_that_moved_and_are_stored() {
        let tiled = [zone(72, 96, None), zone(60, 60, None)];
        assert!(
            moved(&tiled, &[(61, 96), (24, 60)]).is_empty(),
            "nothing moved"
        );

        // The top of the lower zone moved, and the derived low above it followed.
        let sets = moved(&tiled, &[(56, 96), (24, 55)]);
        assert_eq!(sets, [("zone2.top_note".to_string(), "G3".to_string())]);

        // Where the record states its own low, both ends are written.
        let stated = [zone(72, 96, Some(61)), zone(60, 60, Some(24))];
        let sets = moved(&stated, &[(50, 96), (24, 49)]);
        assert_eq!(
            sets,
            [
                ("zone1.low_note".to_string(), "D3".to_string()),
                ("zone2.top_note".to_string(), "C#3".to_string()),
            ]
        );
    }

    /// Clicking a band picks its zone and asks for the row to be brought up under the
    /// map, which is the one thing the map does to the body.
    #[test]
    fn a_click_on_a_band_opens_its_row() {
        let ctx = dressed();
        let snapshot = v2_snapshot();
        let mut state = State::default();
        let (said, _, _) = mapped(&ctx, &mut state, &snapshot, Vec::new());
        assert_eq!(selected(&state), None);

        let band = lowest(&said, "C1–C6");
        let (_, sets, ask) = mapped(&ctx, &mut state, &snapshot, press(band.center()));
        assert_eq!(selected(&state), Some(0));
        assert_eq!(state.reveal, Some(0));
        assert!(sets.is_empty() && ask.is_none(), "a pick is not an edit");
    }

    /// A two-zone v3 body, hand-built to the layout `map` v14 stores: a per-key table
    /// the reader skips, the zone count at the offset the layout fixes, then one
    /// 16-byte record per zone high to low, each holding root, top and low notes and
    /// naming its stroke by global id at offset 8.
    fn v3_sample(version: u32) -> Cbin<nsmp::SampleV3> {
        use nord_format::formats::nsmp::section::{Section4, HDR4, MAP4, STK4};
        use nord_format::formats::nsmp::zone::Wide;

        // `hdr`: the main name at 10, the sub name from 76.
        let mut hdr = vec![0u8; 140];
        hdr[10..23].copy_from_slice(b"Bass Clarinet");
        hdr[76..84].copy_from_slice(b"KG  mono");

        let mut map = vec![0u8; Wide::V14.count_at().unwrap()];
        map.push(2);
        for (gid, root, top, low) in [(2u32, 72u8, 96u8, 61u8), (1, 60, 60, 17)] {
            let mut record = vec![0u8; 16];
            record[0] = root;
            record[1] = top;
            record[2] = low;
            record[8..12].copy_from_slice(&gid.to_be_bytes());
            map.extend(record);
        }

        let stroke = |gid: u32, root: u8| {
            let mut payload = vec![0u8; 68];
            payload[0..4].copy_from_slice(&gid.to_be_bytes());
            payload[5] = root;
            Section4 {
                tag: *STK4,
                version: 9,
                payload,
            }
        };
        Cbin {
            header: nord_format::cbin::Header::new(nsmp::FORMAT, (0, 0), version),
            body: nsmp::SampleV3 {
                sections: vec![
                    Section4 {
                        tag: *HDR4,
                        version: 9,
                        payload: hdr,
                    },
                    Section4 {
                        tag: *MAP4,
                        version: 14,
                        payload: map,
                    },
                    stroke(2, 72),
                    stroke(1, 60),
                ],
            },
        }
    }
}
