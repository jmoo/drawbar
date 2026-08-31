//! The sample-instrument document.
//!
//! A sample is mostly encoded audio, so what is settable is what the format can patch in
//! place without touching a stroke: the name, and each zone's root key and boundaries.
//! Every generation edits. Where a container also describes the keyboard note by note,
//! that description is recomputed from the zones as they move.
//!
//! ⚠️ Decoding a stroke is expensive and a library instrument is hundreds of megabytes,
//! so **nothing here decodes to draw a frame**. A zone's audio is decoded once, when the
//! operator asks for it, and kept in a [`Cache`] until the bytes under it change.

use std::collections::HashMap;
use std::io::Cursor;

use eframe::egui;
use nord_format::formats::nsmp::codec::{self, Audio};
use nord_format::{Entity, Sample};

use super::controls::Sets;
use crate::note;

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
}

/// Everything the document shows about an instrument, in one read.
#[derive(Clone, PartialEq, Eq)]
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
            .map(|zone| Zone {
                root_key: zone.root_key,
                top_note: zone.top_note,
                low_note: zone.low_note,
            })
            .collect(),
        zones_editable: sample.zones_are_editable(),
    })
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

/// Apply every set to a fresh decode and re-encode, the same all-or-nothing rule the
/// registry bodies follow.
pub fn apply(bytes: &[u8], sets: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut entity =
        nord_format::from_stream(&mut Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let sample = sample_mut(&mut entity).ok_or("not a sample instrument")?;
    for (path, value) in sets {
        set(sample, path, value)?;
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
    let low = zones[index].low_note.or_else(|| {
        zones
            .get(index + 1)
            .map(|below| below.top_note.saturating_add(1))
    });
    match low {
        Some(low) => format!("{} up to {top}", note::name(low)),
        None => format!("up to {top}"),
    }
}

/// What the sample view asked the document to do about one zone's audio.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ask {
    /// Decode this zone, because the operator opened it.
    Decode(usize),
    /// Start it, or stop it if it is the one sounding.
    Play(usize),
    Save(usize),
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

/// The name, the zone map, and each zone's audio once it has been asked for.
pub fn ui(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    name: &mut String,
    sounds: &[Sound],
    sets: &mut Sets,
) -> Option<Ask> {
    let mut ask = None;
    ui.horizontal(|ui| {
        ui.add_sized(
            [120.0, ui.spacing().interact_size.y],
            egui::Label::new("Name").halign(egui::Align::LEFT),
        );
        let response = ui.add(
            egui::TextEdit::singleline(name)
                .desired_width(200.0)
                .char_limit(snapshot.max_name_len),
        );
        // Not committed per keystroke: half a name is a name the format would take.
        let done = response.lost_focus() || response.ctx.input(|i| i.key_pressed(egui::Key::Enter));
        if done && *name != snapshot.name {
            sets.push(("name".to_string(), name.clone()));
        }
    });
    if !snapshot.sub_name.is_empty() {
        labelled(ui, "Sub name", &snapshot.sub_name);
    }
    if !snapshot.categories.is_empty() {
        labelled(ui, "Categories", &snapshot.categories.join(", "));
    }
    if !snapshot.zones.is_empty() && !snapshot.zones_editable {
        ui.label(
            egui::RichText::new(
                "This instrument's keyboard map cannot be read, so its zones are shown \
                 rather than changed. The name is still yours to set.",
            )
            .weak(),
        );
    }

    for (i, zone) in snapshot.zones.iter().enumerate() {
        let n = i + 1;
        ui.add_space(4.0);
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
            ui.add_enabled_ui(snapshot.zones_editable, |ui| {
                ui.label("root key");
                if let Some(note) = note_picker(ui, ("root", n), zone.root_key) {
                    sets.push((format!("zone{n}.root_key"), note::name(note)));
                }
                ui.label("top note");
                if let Some(note) = note_picker(ui, ("top", n), zone.top_note) {
                    sets.push((format!("zone{n}.top_note"), note::name(note)));
                }
                if let Some(low) = zone.low_note {
                    ui.label("low note");
                    if let Some(note) = note_picker(ui, ("low", n), low) {
                        sets.push((format!("zone{n}.low_note"), note::name(note)));
                    }
                }
            });
        });
        // Outside the enable gate: audio plays whether or not the zones can be moved.
        if let Some(sound) = sounds.get(i) {
            if let Some(asked) = zone_audio(ui, i, sound) {
                ask = Some(asked);
            }
        }
    }
    ask
}

/// A read-only fact, laid out under the same label column the controls use.
fn labelled(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.add_sized(
            [120.0, ui.spacing().interact_size.y],
            egui::Label::new(label).halign(egui::Align::LEFT),
        );
        ui.label(egui::RichText::new(value).weak());
    });
}

/// What the document knows about one zone's audio while it draws the zone.
pub struct Sound<'a> {
    pub decoded: Option<&'a Result<Decoded, String>>,
    pub playing: bool,
}

fn zone_audio(ui: &mut egui::Ui, index: usize, sound: &Sound) -> Option<Ask> {
    let mut ask = None;
    ui.horizontal_wrapped(|ui| {
        ui.add_space(120.0);
        match sound.decoded {
            None => {
                if ui
                    .small_button("Show audio")
                    .on_hover_text("decode this zone's stroke — a long one takes a moment")
                    .clicked()
                {
                    ask = Some(Ask::Decode(index));
                }
            }
            Some(Err(why)) => {
                ui.label(
                    egui::RichText::new(format!("not decoded: {why}"))
                        .small()
                        .color(crate::app::bad(ui.visuals())),
                );
            }
            Some(Ok(decoded)) => {
                ui.label(
                    egui::RichText::new(format!(
                        "{:.3} s  {}",
                        decoded.audio.seconds(),
                        match decoded.audio.channels {
                            1 => "mono".to_string(),
                            n => format!("{n} channels"),
                        },
                    ))
                    .small()
                    .weak(),
                );
                let label = match sound.playing {
                    true => "Stop",
                    false => "Play",
                };
                if ui.small_button(label).clicked() {
                    ask = Some(Ask::Play(index));
                }
                if ui.small_button("Save WAV…").clicked() {
                    ask = Some(Ask::Save(index));
                }
            }
        }
    });
    if let Some(Ok(decoded)) = sound.decoded {
        ui.horizontal(|ui| {
            ui.add_space(120.0);
            waveform(ui, &decoded.envelope, sound.playing);
        });
    }
    ask
}

/// Height of the drawn envelope.
const WAVE_HEIGHT: f32 = 44.0;

/// Draw an envelope across whatever width is left.
///
/// Painted from the theme's own colours rather than fixed ones: the trough is the panel's
/// extreme fill, the wave is the instrument's red while it is sounding and the body text
/// colour when it is not, so both themes stay legible.
fn waveform(ui: &mut egui::Ui, envelope: &[(f32, f32)], playing: bool) {
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
                low_note: None,
            },
            Zone {
                root_key: 60,
                top_note: 71,
                low_note: None,
            },
        ];
        // Zone 2 tops out at B4, so zone 1 starts one key above it.
        assert_eq!(range(&zones, 0), "C5 up to C7");
        assert_eq!(range(&zones, 1), "up to B4");

        // A file that states its own bottom is believed rather than derived.
        let stated = vec![Zone {
            root_key: 60,
            top_note: 71,
            low_note: Some(48),
        }];
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
    }

    #[test]
    fn a_v4_instrument_says_so() {
        let entity = Entity::Sample(Sample::V3(v3_sample(400)));
        let snapshot = snapshot(&entity).unwrap().unwrap();
        assert_eq!(snapshot.generation, "v4");
    }

    /// A two-zone v3 body, hand-built to the layout `map` v14 stores: the zone count,
    /// then one 16-byte record per zone high to low, each holding root, top and low
    /// notes and naming its stroke by global id at offset 8.
    fn v3_sample(version: u32) -> Cbin<nsmp::SampleV3> {
        use nord_format::formats::nsmp::section::{Section4, HDR4, MAP4, STK4};

        // `hdr`: the main name at 10, the sub name from 76.
        let mut hdr = vec![0u8; 140];
        hdr[10..23].copy_from_slice(b"Bass Clarinet");
        hdr[76..84].copy_from_slice(b"KG  mono");

        let mut map = vec![2u8];
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
