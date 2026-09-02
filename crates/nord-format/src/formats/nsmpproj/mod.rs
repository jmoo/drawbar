//! Nord Sample Editor projects (`.nsmpproj`) — the editor's own save file,
//! from which it generates an [`nsmp`](super::nsmp) instrument.
//!
//! A project is plain text: one `SMACEditorProject { … }` [`tree`] of
//! `key = value` fields and nested blocks, which this module reads and writes
//! byte-exactly and exposes through typed views. The layout is inferred from
//! projects the editor saved, and the editor loads what [`Project::new`] writes.
//!
//! The tree, from the root down:
//!
//! - `document` — counts of what follows.
//! - `audio_file` × N — the WAV files, by id: path as the editor saw it
//!   (relative to the project), sample rate.
//! - `common_zone` × N — one per zone, **high to low**, each holding one
//!   `common_stroke`: the stroke's global id, the audio file it plays, its
//!   trim and loop points in frames.
//! - `instrument` — the one instrument every specimen holds: its name, a
//!   `samplib_attrs` block of the instrument-side defaults, EQ, an
//!   `instrument_zone` × N echo of the zones, and one `map_info` whose
//!   `map_zone` × N (high to low) carry each zone's root key, key range and
//!   per-stroke gain, detune and velocity window, and whose 92 `note_info`
//!   blocks cover MIDI notes 17–108.
//!
//! Every `*.size` field restates the count of the blocks after it. Zone ids
//! start at 129 and rise with the root key; stroke global ids and audio-file ids
//! are the same numbers, rising from 1 in the same order.
//!
//! Frame positions are stored as `%f` decimals. Inferred from specimens: they
//! count at 44 100 Hz whatever `m_sampleRate` says — a 0.1 s file stores
//! `m_end = 4410` at 22 050 Hz and 96 000 Hz alike.

pub mod tree;

pub use tree::{Entry, Node};

use crate::error::{Error, ParseError};
use crate::formats::nsmp::zone::derive_top_notes;
use std::io::{Read, Write};

/// The first bytes of every project: the root block's opening line.
pub const MAGIC: &[u8] = b"SMACEditorProject {";

/// The extension the editor saves under; this crate's name for the format.
pub const FORMAT: &str = "nsmpproj";

/// The `m_fileFormatVersion` every specimen carries. Not gated on read: keys
/// are named, so a different version cannot be misread — a view that needs a
/// key the file lacks fails naming it.
pub const KNOWN_FILE_FORMAT_VERSION: u32 = 54;

/// The lowest key every specimen maps: the bottom zone always reaches F0.
pub const LOWEST_NOTE: u8 = 17;

/// The highest key `note_info` describes (C8); the editor lays out
/// 92 of them, `LOWEST_NOTE..=HIGHEST_NOTE`.
pub const HIGHEST_NOTE: u8 = 108;

/// The id of the lowest zone; ids rise with the root key.
pub const FIRST_ZONE_ID: u32 = 129;

/// Lowest secondary start the editor keeps, in frames. Below it a stroke's
/// `m_startSecondary` is repaired on load, see [`repaired_secondary_start`].
/// Inferred from specimens; not confirmed on hardware.
pub const MIN_SECONDARY_START: f64 = 92.0;

/// The `m_startSecondary` a fresh project states for a stroke over `end` frames.
///
/// The editor's own value is an attack analysis within a percent of this on every
/// specimen; this is what [`Project::new`] writes, and the editor keeps it.
pub fn default_secondary_start(end: f64) -> f64 {
    end / 8.0
}

/// The secondary start the editor encodes from, given what the project states.
///
/// On load the editor keeps `stated` when it lies between [`MIN_SECONDARY_START`] and
/// a ceiling — half of `stop`, or the loop start when that is lower and the loop is
/// switched on — and otherwise replaces it with half the ceiling, floored at
/// [`MIN_SECONDARY_START`]. A 441-frame stroke stating 1 encodes from 110.25.
/// Every position is in the file's frames.
/// Inferred from specimens; not confirmed on hardware.
pub fn repaired_secondary_start(stated: f64, stop: f64, loop_start: Option<f64>) -> f64 {
    let ceiling = loop_start.map_or(stop / 2.0, |start| start.min(stop / 2.0));
    if (MIN_SECONDARY_START..=ceiling).contains(&stated) {
        stated
    } else {
        (ceiling / 2.0).max(MIN_SECONDARY_START)
    }
}

/// One saved project. Reads and writes byte-exactly; the views decode the
/// tree on demand and the setters edit it in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: Node,
}

/// One `audio_file` block.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFile {
    pub id: u32,
    /// The path as the editor stored it — relative to the project's directory
    /// in every specimen.
    pub path: String,
    pub sample_rate: u32,
}

/// One `common_stroke`: which file a zone plays and where in it.
///
/// Positions are frames, see the module doc.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    /// The `common_zone` this stroke sits in.
    pub zone_id: u32,
    pub global_id: u32,
    /// The [`AudioFile`] it plays.
    pub file_id: u32,
    pub begin: f64,
    pub end: f64,
    pub start: f64,
    /// `m_startSecondary`: the attack analysis the editor stores, in file frames. The
    /// encoded stream resynchronises here, measured from [`start`](Stroke::start) —
    /// after the repair in [`encoded_secondary_start`](Stroke::encoded_secondary_start).
    pub start_secondary: f64,
    pub stop: f64,
    pub loop_enabled: bool,
    pub loop_start: f64,
    /// `m_loopLengthLong`.
    pub loop_length: f64,
    /// `m_loopXFadeLengthLong`, in frames. The editor bakes this fade into the
    /// encoded audio; nothing in the instrument states it.
    pub loop_crossfade: f64,
    /// `m_loopXFModeLong`. Mode 0 is the linear fade; what mode 1 does to the audio
    /// is not decoded.
    pub loop_crossfade_mode: u32,
    /// `m_loopDecayEnabled`. Reaches the instrument nowhere.
    pub loop_decay_enabled: bool,
    /// `m_loopDecay`. Reaches the instrument nowhere.
    pub loop_decay: f64,
    /// `m_loopDetune`. Reaches the instrument nowhere.
    pub loop_detune: i32,
    /// `m_shortLoopEnabled`: the short loop starts where the long one does and runs
    /// for [`short_loop_length`](Stroke::short_loop_length) instead.
    pub short_loop_enabled: bool,
    /// `m_loopLengthShort`.
    pub short_loop_length: f64,
    /// `m_loopXFadeShort` — the short loop's crossfade as a **percentage of
    /// [`short_loop_length`](Stroke::short_loop_length)**, where the long loop's
    /// [`loop_crossfade`](Stroke::loop_crossfade) is a frame count outright. Values above
    /// 100 fade for longer than the loop lasts; the editor does not clamp them.
    /// Inferred from specimens; not confirmed on hardware.
    pub short_loop_crossfade: u32,
    /// `m_shortLoopUsesPitch`. Reaches the instrument nowhere.
    pub short_loop_uses_pitch: bool,
}

impl Stroke {
    /// The secondary start the editor encodes this stroke from, in file frames — see
    /// [`repaired_secondary_start`].
    pub fn encoded_secondary_start(&self) -> f64 {
        repaired_secondary_start(
            self.start_secondary,
            self.stop,
            self.loop_enabled.then_some(self.loop_start),
        )
    }
}

/// One `map_zone`: a root key, the key range it answers to, and its strokes.
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub zone_id: u32,
    pub root_key: u8,
    pub enabled: bool,
    pub bottom_note: u8,
    pub top_note: u8,
    pub strokes: Vec<ZoneStroke>,
}

/// One `map_stroke`: how a zone plays one of its strokes.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneStroke {
    pub global_id: u32,
    pub enabled: bool,
    pub gain: f64,
    pub detune: i32,
    /// `m_velocityMin..=m_velocityMax`.
    pub velocity: (u8, u8),
}

/// The instrument's velocity defaults, from `samplib_attrs`.
///
/// Inferred from specimens; not confirmed on hardware.
///
/// Each value is carried verbatim: nothing observed distinguishes a flag from a depth,
/// so a rewrite must not collapse one to 0 or 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VelocityDefaults {
    /// `m_atkVelocityAmount`.
    pub attack_amount: u8,
    /// `m_velAmpl`.
    pub amplitude: u8,
    /// `m_velTimbre`.
    pub timbre: u8,
}

/// The highest velocity a window end may name.
pub const MAX_VELOCITY: u8 = 127;

/// One field of one stroke, with the value to give it.
///
/// The trim and loop points sit in the `common_zone`'s `common_stroke`; gain
/// and the velocity window in the `map_zone`'s `map_stroke`. Both blocks name
/// the stroke by the same `m_globalID`, so one id reaches either.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StrokeField {
    /// `m_start`: where playback begins, frames into the file.
    Start(f64),
    /// `m_stop`: where it ends.
    Stop(f64),
    LoopEnabled(bool),
    LoopStart(f64),
    /// `m_loopLengthLong`.
    LoopLength(f64),
    /// `m_loopXFadeLengthLong`, frames.
    LoopCrossfade(f64),
    /// `m_loopXFModeLong`.
    LoopCrossfadeMode(u32),
    LoopDecayEnabled(bool),
    LoopDecay(f64),
    LoopDetune(i32),
    ShortLoopEnabled(bool),
    /// `m_loopLengthShort`.
    ShortLoopLength(f64),
    /// `m_loopXFadeShort`.
    ShortLoopCrossfade(u32),
    ShortLoopUsesPitch(bool),
    /// `m_gain`, a linear factor — the editor writes 1 for untouched.
    Gain(f64),
    /// `m_velocityMin`. Not checked against the window's other end: an
    /// inverted window silences the stroke, which is a thing to store.
    VelocityMin(u8),
    /// `m_velocityMax`, under the same rule as [`StrokeField::VelocityMin`].
    VelocityMax(u8),
}

/// Which of the two blocks naming a stroke holds a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    Common,
    Map,
}

impl StrokeField {
    /// One `field = value` in the vocabulary [`Stroke`] and [`ZoneStroke`]
    /// spell, so a CLI, a UI and a script name a stroke's fields the same way.
    /// Flags take `on`/`off`, `yes`/`no`, `true`/`false` or `1`/`0`.
    pub fn parse(field: &str, value: &str) -> Result<StrokeField, ParseError> {
        use StrokeField::*;
        Ok(match field {
            "start" => Start(number(field, value)?),
            "stop" => Stop(number(field, value)?),
            "gain" => Gain(number(field, value)?),
            "velocity_min" => VelocityMin(number(field, value)?),
            "velocity_max" => VelocityMax(number(field, value)?),
            "loop_enabled" => LoopEnabled(truth(field, value)?),
            "loop_start" => LoopStart(number(field, value)?),
            "loop_length" => LoopLength(number(field, value)?),
            "loop_crossfade" => LoopCrossfade(number(field, value)?),
            "loop_crossfade_mode" => LoopCrossfadeMode(number(field, value)?),
            "loop_decay_enabled" => LoopDecayEnabled(truth(field, value)?),
            "loop_decay" => LoopDecay(number(field, value)?),
            "loop_detune" => LoopDetune(number(field, value)?),
            "short_loop_enabled" => ShortLoopEnabled(truth(field, value)?),
            "short_loop_length" => ShortLoopLength(number(field, value)?),
            "short_loop_crossfade" => ShortLoopCrossfade(number(field, value)?),
            "short_loop_uses_pitch" => ShortLoopUsesPitch(truth(field, value)?),
            _ => {
                return Err(ParseError::AssertFail(format!(
                    "a stroke has no field named {field:?}"
                )))
            }
        })
    }

    /// The block, key and text this field writes, or why the value cannot be
    /// written at all.
    fn placement(self) -> Result<(Block, &'static str, String), ParseError> {
        use Block::{Common, Map};
        use StrokeField::*;
        Ok(match self {
            Start(v) => (Common, "m_start", frames(v)?),
            Stop(v) => (Common, "m_stop", frames(v)?),
            LoopEnabled(v) => (Common, "m_loopEnabled", bit(v)),
            LoopStart(v) => (Common, "m_loopStart", frames(v)?),
            LoopLength(v) => (Common, "m_loopLengthLong", frames(v)?),
            LoopCrossfade(v) => (Common, "m_loopXFadeLengthLong", frames(v)?),
            LoopCrossfadeMode(v) => (Common, "m_loopXFModeLong", v.to_string()),
            LoopDecayEnabled(v) => (Common, "m_loopDecayEnabled", bit(v)),
            LoopDecay(v) => (
                Common,
                "m_loopDecay",
                positive(v, "a decay at or above zero")?,
            ),
            LoopDetune(v) => (Common, "m_loopDetune", v.to_string()),
            ShortLoopEnabled(v) => (Common, "m_shortLoopEnabled", bit(v)),
            ShortLoopLength(v) => (Common, "m_loopLengthShort", frames(v)?),
            ShortLoopCrossfade(v) => (Common, "m_loopXFadeShort", v.to_string()),
            ShortLoopUsesPitch(v) => (Common, "m_shortLoopUsesPitch", bit(v)),
            Gain(v) => (Map, "m_gain", positive(v, "a gain at or above zero")?),
            VelocityMin(v) => (Map, "m_velocityMin", velocity(v)?),
            VelocityMax(v) => (Map, "m_velocityMax", velocity(v)?),
        })
    }
}

/// What [`Project::new`] needs for one zone: a WAV and the key it was
/// recorded at.
#[derive(Debug, Clone, PartialEq)]
pub struct NewZone {
    /// The audio file's path, as the editor should find it — relative paths
    /// resolve from the project's directory.
    pub path: String,
    pub sample_rate: u32,
    /// Frames in the file, at 44 100 Hz (see the module doc).
    pub frames: u64,
    pub root_key: u8,
}

fn flag(node: &Node, key: &str) -> Result<bool, ParseError> {
    Ok(node.get::<u8>(key)? != 0)
}

/// `%f`: six decimals, the way the editor writes every real number.
fn real(v: f64) -> String {
    format!("{v:.6}")
}

fn bit(v: bool) -> String {
    if v { "1" } else { "0" }.to_string()
}

fn positive(v: f64, bound: &str) -> Result<String, ParseError> {
    if !v.is_finite() || v < 0.0 {
        return Err(ParseError::OutOfBounds {
            value: v.to_string(),
            bound: bound.into(),
        });
    }
    Ok(real(v))
}

fn frames(v: f64) -> Result<String, ParseError> {
    positive(v, "a frame position at or above zero")
}

fn number<T: std::str::FromStr>(field: &str, value: &str) -> Result<T, ParseError> {
    value.parse().map_err(|_| {
        ParseError::AssertFail(format!(
            "{field} = {value:?} is not a {}",
            std::any::type_name::<T>()
        ))
    })
}

fn truth(field: &str, value: &str) -> Result<bool, ParseError> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        _ => Err(ParseError::AssertFail(format!(
            "{field} = {value:?} is not on or off"
        ))),
    }
}

fn velocity(v: u8) -> Result<String, ParseError> {
    if v > MAX_VELOCITY {
        return Err(ParseError::OutOfBounds {
            value: v.to_string(),
            bound: format!("0..={MAX_VELOCITY}"),
        });
    }
    Ok(v.to_string())
}

impl Project {
    pub fn read_from(reader: &mut impl Read) -> Result<Project, Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        if !bytes.starts_with(MAGIC) {
            return Err(
                ParseError::UnknownFileType("not a Nord Sample Editor project".into()).into(),
            );
        }
        let text = String::from_utf8(bytes).map_err(|e| {
            ParseError::AssertFail(format!(
                "project is not UTF-8 at byte {}",
                e.utf8_error().valid_up_to()
            ))
        })?;
        Ok(Project::parse(&text)?)
    }

    pub fn parse(text: &str) -> Result<Project, ParseError> {
        let root = tree::parse(text)?;
        if root.name != "SMACEditorProject" {
            return Err(ParseError::AssertFail(format!(
                "root block is {}, not SMACEditorProject",
                root.name
            )));
        }
        Ok(Project { root })
    }

    pub fn write_to(&self, writer: &mut impl Write) -> Result<(), Error> {
        writer.write_all(self.render().as_bytes())?;
        Ok(())
    }

    /// The file's text.
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.root.render(&mut out, 0);
        out
    }

    pub fn file_format_version(&self) -> Result<u32, ParseError> {
        self.root.get("m_fileFormatVersion")
    }

    /// `(product, version)` — `("Nord Sample Editor", "v4.36_b1616")` from the
    /// editor.
    pub fn created_by(&self) -> Result<(String, String), ParseError> {
        Ok((
            self.root.get("m_createdByProdName")?,
            self.root.get("m_createdByProdVer")?,
        ))
    }

    pub fn audio_files(&self) -> Result<Vec<AudioFile>, ParseError> {
        self.root
            .blocks("audio_file")
            .map(|n| {
                Ok(AudioFile {
                    id: n.get("m_id")?,
                    path: n.get("m_fullName")?,
                    sample_rate: n.get("m_sampleRate")?,
                })
            })
            .collect()
    }

    /// Every `common_stroke`, in file order — zones high to low.
    pub fn strokes(&self) -> Result<Vec<Stroke>, ParseError> {
        let mut out = Vec::new();
        for zone in self.root.blocks("common_zone") {
            let zone_id = zone.get("m_zoneId")?;
            for s in zone.blocks("common_stroke") {
                out.push(Stroke {
                    zone_id,
                    global_id: s.get("m_globalID")?,
                    file_id: s.get("m_fileID")?,
                    begin: s.get("m_begin")?,
                    end: s.get("m_end")?,
                    start: s.get("m_start")?,
                    start_secondary: s.get("m_startSecondary")?,
                    stop: s.get("m_stop")?,
                    loop_enabled: flag(s, "m_loopEnabled")?,
                    loop_start: s.get("m_loopStart")?,
                    loop_length: s.get("m_loopLengthLong")?,
                    loop_crossfade: s.get("m_loopXFadeLengthLong")?,
                    loop_crossfade_mode: s.get("m_loopXFModeLong")?,
                    loop_decay_enabled: flag(s, "m_loopDecayEnabled")?,
                    loop_decay: s.get("m_loopDecay")?,
                    loop_detune: s.get("m_loopDetune")?,
                    short_loop_enabled: flag(s, "m_shortLoopEnabled")?,
                    short_loop_length: s.get("m_loopLengthShort")?,
                    short_loop_crossfade: s.get("m_loopXFadeShort")?,
                    short_loop_uses_pitch: flag(s, "m_shortLoopUsesPitch")?,
                });
            }
        }
        Ok(out)
    }

    /// The instrument block. Every specimen holds exactly one.
    fn instrument(&self) -> Result<&Node, ParseError> {
        self.root.require("instrument")
    }

    fn instrument_mut(&mut self) -> Result<&mut Node, ParseError> {
        self.root
            .blocks_mut("instrument")
            .next()
            .ok_or_else(|| ParseError::AssertFail("project has no instrument block".into()))
    }

    /// The instrument's name — what the generated `.nsmp` is called.
    pub fn name(&self) -> Result<String, ParseError> {
        self.instrument()?.get("m_name")
    }

    /// `m_loopDecayEnabled` on the instrument, which is a different object from the
    /// one on each stroke. Reaches the generated instrument nowhere.
    pub fn loop_decay_enabled(&self) -> Result<bool, ParseError> {
        flag(self.instrument()?, "m_loopDecayEnabled")
    }

    pub fn set_name(&mut self, name: &str) -> Result<(), ParseError> {
        self.instrument_mut()?.set_field("m_name", name)
    }

    /// The instrument's zones, in file order — high to low.
    pub fn zones(&self) -> Result<Vec<Zone>, ParseError> {
        let map = self.instrument()?.require("map_info")?;
        map.blocks("map_zone")
            .map(|z| {
                Ok(Zone {
                    zone_id: z.get("m_zoneId")?,
                    root_key: z.get("m_rootKey")?,
                    enabled: flag(z, "m_isEnabled")?,
                    bottom_note: z.get("m_btmNote")?,
                    top_note: z.get("m_topNote")?,
                    strokes: z
                        .blocks("map_stroke")
                        .map(|s| {
                            Ok(ZoneStroke {
                                global_id: s.get("m_globalID")?,
                                enabled: flag(s, "m_isEnabled")?,
                                gain: s.get("m_gain")?,
                                detune: s.get("m_detune")?,
                                velocity: (s.get("m_velocityMin")?, s.get("m_velocityMax")?),
                            })
                        })
                        .collect::<Result<_, ParseError>>()?,
                })
            })
            .collect()
    }

    fn map_info_mut(&mut self) -> Result<&mut Node, ParseError> {
        self.instrument_mut()?
            .blocks_mut("map_info")
            .next()
            .ok_or_else(|| ParseError::AssertFail("instrument has no map_info block".into()))
    }

    fn map_zone_mut(&mut self, zone_id: u32) -> Result<&mut Node, ParseError> {
        let map = self.map_info_mut()?;
        map.blocks_mut("map_zone")
            .find(|z| z.field("m_zoneId") == Some(zone_id.to_string().as_str()))
            .ok_or_else(|| ParseError::AssertFail(format!("no zone with id {zone_id}")))
    }

    pub fn set_root_key(&mut self, zone_id: u32, key: u8) -> Result<(), ParseError> {
        self.map_zone_mut(zone_id)?
            .set_field("m_rootKey", key.to_string())
    }

    /// Set a zone's key range. Nothing checks it against the neighbours: the
    /// editor stores whatever it is given and repairs overlaps on load.
    pub fn set_key_range(&mut self, zone_id: u32, bottom: u8, top: u8) -> Result<(), ParseError> {
        if bottom > top {
            return Err(ParseError::OutOfBounds {
                value: format!("{bottom}..={top}"),
                bound: "a key range with its bottom at or below its top".into(),
            });
        }
        let zone = self.map_zone_mut(zone_id)?;
        zone.set_field("m_btmNote", bottom.to_string())?;
        zone.set_field("m_topNote", top.to_string())
    }

    /// The instrument's `samplib_attrs` block.
    fn attrs(&self) -> Result<&Node, ParseError> {
        self.instrument()?.require("samplib_attrs")
    }

    fn attrs_mut(&mut self) -> Result<&mut Node, ParseError> {
        self.instrument_mut()?
            .blocks_mut("samplib_attrs")
            .next()
            .ok_or_else(|| ParseError::AssertFail("instrument has no samplib_attrs block".into()))
    }

    pub fn velocity_defaults(&self) -> Result<VelocityDefaults, ParseError> {
        let attrs = self.attrs()?;
        Ok(VelocityDefaults {
            attack_amount: attrs.get("m_atkVelocityAmount")?,
            amplitude: attrs.get("m_velAmpl")?,
            timbre: attrs.get("m_velTimbre")?,
        })
    }

    pub fn set_velocity_defaults(&mut self, v: VelocityDefaults) -> Result<(), ParseError> {
        let attrs = self.attrs_mut()?;
        let fields = [
            ("m_atkVelocityAmount", v.attack_amount.to_string()),
            ("m_velAmpl", v.amplitude.to_string()),
            ("m_velTimbre", v.timbre.to_string()),
        ];
        for (key, _) in &fields {
            if attrs.field(key).is_none() {
                return Err(ParseError::AssertFail(format!(
                    "{} has no {key}",
                    attrs.name
                )));
            }
        }
        for (key, value) in fields {
            attrs.set_field(key, value)?;
        }
        Ok(())
    }

    /// The `common_stroke` and `map_stroke` a global id names.
    fn stroke_mut(&mut self, global_id: u32, block: Block) -> Result<&mut Node, ParseError> {
        let want = global_id.to_string();
        let (zones, inner) = match block {
            Block::Common => (self.root.blocks_mut("common_zone"), "common_stroke"),
            Block::Map => (self.map_info_mut()?.blocks_mut("map_zone"), "map_stroke"),
        };
        zones
            .flat_map(|zone| Node::blocks_mut(zone, inner))
            .find(|s| s.field("m_globalID") == Some(want.as_str()))
            .ok_or_else(|| ParseError::AssertFail(format!("no {inner} with global id {global_id}")))
    }

    /// Move one field of one stroke, leaving every other byte where it was.
    pub fn set_stroke_field(
        &mut self,
        global_id: u32,
        field: StrokeField,
    ) -> Result<(), ParseError> {
        let (block, key, value) = field.placement()?;
        self.stroke_mut(global_id, block)?.set_field(key, value)
    }

    pub fn set_audio_path(&mut self, file_id: u32, path: &str) -> Result<(), ParseError> {
        self.root
            .blocks_mut("audio_file")
            .find(|f| f.field("m_id") == Some(file_id.to_string().as_str()))
            .ok_or_else(|| ParseError::AssertFail(format!("no audio file with id {file_id}")))?
            .set_field("m_fullName", path)
    }

    /// A project laid out the way the editor lays one out after *Import
    /// Auto…*: one zone per file, zone ids from [`FIRST_ZONE_ID`] rising with
    /// the root key, key ranges from [`derive_top_notes`] down to
    /// [`LOWEST_NOTE`], every other field at the value the editor writes for
    /// an untouched import.
    ///
    /// `modified` is the Unix time stamped on every `m_modifyDate`.
    ///
    /// Unexplained: `m_crc` and `m_crcProj` (the editor's checksum of the
    /// generated instrument — algorithm unknown) are written as 0, and
    /// `m_startSecondary` (an analysis result the editor stores, within a
    /// percent of `end / 8` in every specimen) as exactly that. Confirmed in
    /// Nord Sample Editor 3: it opens such a project (and asks for the audio
    /// files if they are not where the paths say), repairing derived state on
    /// load.
    pub fn new(name: &str, zones: &[NewZone], modified: u32) -> Result<Project, ParseError> {
        if zones.is_empty() {
            return Err(ParseError::AssertFail(
                "a project needs at least one zone".into(),
            ));
        }
        let mut by_root: Vec<&NewZone> = zones.iter().collect();
        by_root.sort_by_key(|z| z.root_key);
        if by_root.windows(2).any(|w| w[0].root_key == w[1].root_key) {
            return Err(ParseError::AssertFail("two zones share a root key".into()));
        }
        if !(LOWEST_NOTE..=HIGHEST_NOTE).contains(&by_root[0].root_key)
            || !(LOWEST_NOTE..=HIGHEST_NOTE).contains(&by_root[by_root.len() - 1].root_key)
        {
            return Err(ParseError::OutOfBounds {
                value: "root key".into(),
                bound: format!("{LOWEST_NOTE}..={HIGHEST_NOTE}"),
            });
        }
        // Low to high: (id, zone). Blocks are emitted high to low.
        let numbered: Vec<(u32, &NewZone)> = by_root
            .iter()
            .enumerate()
            .map(|(i, z)| (i as u32 + 1, *z))
            .collect();
        let n = numbered.len();
        let count = n.to_string();
        let date = modified.to_string();

        let roots_high_to_low: Vec<u8> = numbered.iter().rev().map(|(_, z)| z.root_key).collect();
        let tops = derive_top_notes(&roots_high_to_low);
        // Bottom of zone i (high to low) is one above the top of the zone below it.
        let bottoms: Vec<u8> = (0..n)
            .map(|i| tops.get(i + 1).map_or(LOWEST_NOTE, |t| t + 1))
            .collect();

        let mut root = Node::new("SMACEditorProject");
        root.push_field("m_fileFormatVersion", KNOWN_FILE_FORMAT_VERSION.to_string());
        root.push_field("m_createdByProdName", "nord-format");
        root.push_field(
            "m_createdByProdVer",
            concat!("v", env!("CARGO_PKG_VERSION")),
        );

        let mut document = Node::new("document");
        document.push_field("m_zones.size", &count);
        document.push_field("m_instruments.size", "1");
        document.push_field("m_audioFiles.size", &count);
        document.push_field("m_maxStrength", "0");
        document.push_field("m_minStrength", "0");
        document.push_field("m_modifyDate", &date);
        root.push_block(document);

        for (id, z) in &numbered {
            let mut file = Node::new("audio_file");
            file.push_field("m_id", id.to_string());
            file.push_field("m_fullName", &z.path);
            file.push_field("m_sampleRate", z.sample_rate.to_string());
            file.push_field("m_modifyDate", &date);
            root.push_block(file);
        }

        for (id, z) in numbered.iter().rev() {
            let mut zone = Node::new("common_zone");
            zone.push_field("m_zoneId", (FIRST_ZONE_ID + id - 1).to_string());
            zone.push_field("m_strokes.size", "1");
            zone.push_field("m_modifyDate", &date);
            zone.push_field("m_mapTopNote", "0");
            zone.push_block(common_stroke(*id, z.frames, &date));
            root.push_block(zone);
        }

        let mut instrument = Node::new("instrument");
        instrument.push_field("m_id", "0");
        instrument.push_field("m_type", "0");
        instrument.push_field("m_name", name);
        instrument.push_field("m_crc", "0");
        instrument.push_field("m_crcProj", "0");
        instrument.push_field("m_revision", "0");
        instrument.push_field("m_zones.size", &count);
        instrument.push_field("m_maps.size", "1");
        instrument.push_field("m_modifyDate", &date);
        instrument.push_field("m_loopDecayEnabled", "0");
        instrument.push_field("m_loopDecayHi", "20.000000");
        instrument.push_field("m_loopDecayLo", "20.000000");
        let mut default_params = Node::new("default_params");
        default_params.push_field("m_bufferSize", "0");
        default_params.push_field("m_buffer", "");
        instrument.push_block(default_params);
        let mut attrs = Node::new("samplib_attrs");
        for (key, value) in SAMPLIB_ATTRS {
            attrs.push_field(*key, *value);
        }
        instrument.push_block(attrs);
        for (key, value) in INSTRUMENT_CATEGORY_AND_EQ {
            instrument.push_field(*key, *value);
        }
        for (id, _) in numbered.iter().rev() {
            let mut zone = Node::new("instrument_zone");
            zone.push_field("m_zoneId", (FIRST_ZONE_ID + id - 1).to_string());
            zone.push_field("m_strokes.size", "1");
            zone.push_field("m_modifyDate", &date);
            let mut stroke = Node::new("instrument_stroke");
            stroke.push_field("m_globalID", id.to_string());
            stroke.push_field("m_modifyDate", &date);
            zone.push_block(stroke);
            instrument.push_block(zone);
        }

        let mut map = Node::new("map_info");
        map.push_field("m_id", "0");
        map.push_field("m_name", "Map");
        map.push_field("m_gain", "1.000000");
        map.push_field("m_detune", "0");
        map.push_field("m_files.size", &count);
        map.push_field("m_notes.size", (HIGHEST_NOTE - LOWEST_NOTE + 1).to_string());
        map.push_field("m_zones.size", &count);
        map.push_field("m_modifyDate", &date);
        for i in 0..8 {
            map.push_field(format!("m_macroSpinCtrlVal[{i}]"), "1.000000");
        }
        map.push_field("m_normalize", "0");
        map.push_field("m_emphasisMode", "0");
        for (id, _) in &numbered {
            let mut file = Node::new("map_audiofile");
            file.push_field("m_id", id.to_string());
            file.push_field("m_isEnabled", "1");
            file.push_field("m_modifyDate", &date);
            map.push_block(file);
        }
        for note in LOWEST_NOTE..=HIGHEST_NOTE {
            let mut info = Node::new("note_info");
            info.push_field("m_no", note.to_string());
            info.push_field("m_gain", "1.000000");
            info.push_field("m_detune", "0");
            info.push_field("m_modifyDate", &date);
            map.push_block(info);
        }
        for (i, (id, z)) in numbered.iter().rev().enumerate() {
            let mut zone = Node::new("map_zone");
            zone.push_field("m_zoneId", (FIRST_ZONE_ID + id - 1).to_string());
            zone.push_field("m_rootKey", z.root_key.to_string());
            zone.push_field("m_isEnabled", "1");
            zone.push_field("m_btmNote", bottoms[i].to_string());
            zone.push_field("m_topNote", tops[i].to_string());
            for (key, value) in MAP_ZONE_DEFAULTS {
                zone.push_field(*key, *value);
            }
            zone.push_field("m_strokes.size", "1");
            zone.push_field("m_modifyDate", &date);
            let mut stroke = Node::new("map_stroke");
            stroke.push_field("m_globalID", id.to_string());
            for (key, value) in MAP_STROKE_DEFAULTS {
                stroke.push_field(*key, *value);
            }
            stroke.push_field("m_modifyDate", &date);
            zone.push_block(stroke);
            map.push_block(zone);
        }
        instrument.push_block(map);
        root.push_block(instrument);

        Ok(Project { root })
    }
}

/// A `common_stroke` over a whole file, with the loop points the editor
/// derives for an untouched import: the loop starts halfway, runs to one frame
/// short of the end, and cross-fades over 15% of its length.
///
/// Inferred from specimens. A few hold a loop start half a frame above
/// `end / 2` — an analysis result, like `m_startSecondary`, that nothing here
/// reproduces.
fn common_stroke(global_id: u32, frames: u64, date: &str) -> Node {
    let end = frames as f64;
    let start = 1.0;
    let loop_start = end / 2.0;
    let loop_length = loop_start - 1.0;
    let mut s = Node::new("common_stroke");
    s.push_field("m_globalID", global_id.to_string());
    s.push_field("m_fileID", global_id.to_string());
    s.push_field("m_strokeType", "0");
    s.push_field("m_strength", "0");
    s.push_field("m_shortLoopUsesPitch", "1");
    s.push_field("m_begin", real(0.0));
    s.push_field("m_end", real(end));
    s.push_field("m_start", real(start));
    s.push_field("m_startSecondary", real(default_secondary_start(end)));
    s.push_field("m_stop", real(end));
    s.push_field("m_loopEnabled", "0");
    s.push_field("m_shortLoopEnabled", "0");
    s.push_field("m_loopLengthShort", real(0.0));
    s.push_field("m_loopXFadeShort", "10");
    s.push_field("m_loopStart", real(loop_start));
    s.push_field("m_loopLengthLong", real(loop_length));
    s.push_field("m_loopDecayEnabled", "0");
    s.push_field("m_loopDecay", real(20.0));
    s.push_field("m_loopDetune", "0");
    s.push_field("m_loopXFadeLengthLong", real(loop_length * 0.15));
    s.push_field("m_loopXFModeLong", "0");
    s.push_field("m_release", real(0.0));
    s.push_field("m_fadeInEnable", "0");
    s.push_field("m_fadeInLength", real(9600.0));
    s.push_field("m_fadeOutLength", real(9600.0));
    s.push_field("m_fadeOutRef", "0");
    s.push_field("m_fadeOutMode", "0");
    s.push_field("m_fadeOutEnable", "0");
    s.push_field("m_modifyDate", date);
    s
}

const SAMPLIB_ATTRS: &[(&str, &str)] = &[
    ("m_decay", "5"),
    ("m_release", "30"),
    ("m_monoMode", "0"),
    ("m_dynamics", "0"),
    ("m_velAmpl", "1"),
    ("m_velTimbre", "1"),
    ("m_octaveShift", "0"),
    ("m_atkVelocityAmount", "0"),
    ("m_ne3SlowAttackMode", "0"),
    ("m_fltSlope", "0"),
    ("m_phaseVar", "0"),
    ("m_rndStart", "0"),
    ("m_fltKBT", "1"),
    ("m_voiceMode", "0"),
];

const INSTRUMENT_CATEGORY_AND_EQ: &[(&str, &str)] = &[
    ("m_categoryCategory", "15"),
    ("m_categorySubCategory", "0"),
    ("m_categoryTimbre", "0"),
    ("m_categoryEnvelope", "0"),
    ("m_categoryMotion", "1"),
    ("m_categoryProduction", "Production"),
    ("m_categoryOrigin", "Origin"),
    ("m_categoryDynamicsEnable", "0"),
    ("m_eqLowCutFreq", "50.000000"),
    ("m_eqLowCutEnable", "0"),
    ("m_eqMidGain_0", "0.000000"),
    ("m_eqMidGain_1", "0.000000"),
    ("m_eqMidFreq_0", "1000.000000"),
    ("m_eqMidFreq_1", "4000.000000"),
    ("m_eqMidQ_0", "0.500000"),
    ("m_eqMidQ_1", "0.500000"),
    ("m_eqMidEnable_0", "0"),
    ("m_eqMidEnable_1", "0"),
];

/// A `map_zone`'s fields after the key range and before its strokes.
const MAP_ZONE_DEFAULTS: &[(&str, &str)] = &[
    ("m_zoneMode", "0"),
    ("m_zonePlayback", "0"),
    ("m_isOneShot", "0"),
    ("m_ImagingEnable", "0"),
    ("m_ImagingL", "1.000000"),
    ("m_ImagingR2L", "0.000000"),
    ("m_ImagingL2R", "0.000000"),
    ("m_ImagingR", "1.000000"),
    ("m_eqLowCutFreq", "50.000000"),
    ("m_eqLowCutEnable", "0"),
    ("m_eqMidGain_0", "0.000000"),
    ("m_eqMidGain_1", "0.000000"),
    ("m_eqMidFreq_0", "1000.000000"),
    ("m_eqMidFreq_1", "4000.000000"),
    ("m_eqMidQ_0", "0.500000"),
    ("m_eqMidQ_1", "0.500000"),
    ("m_eqMidEnable_0", "0"),
    ("m_eqMidEnable_1", "0"),
    ("m_eqHiCutFreq", "10000.000000"),
    ("m_eqHiCutEnable", "0"),
    ("m_eqMidGain_2", "0.000000"),
    ("m_eqMidFreq_2", "6000.000000"),
    ("m_eqMidQ_2", "0.500000"),
    ("m_eqMidEnable_2", "0"),
    ("m_eqMidDynGainEn_0", "0"),
    ("m_eqMidDynGainEn_1", "0"),
    ("m_eqMidDynGainEn_2", "0"),
    ("m_eqMidVarGainEn_0", "0"),
    ("m_eqMidVarGainEn_1", "0"),
    ("m_eqMidVarGainEn_2", "0"),
    ("m_eqMidVarGainAtcTime_0", "0.000000"),
    ("m_eqMidVarGainAtcTime_1", "0.000000"),
    ("m_eqMidVarGainAtcTime_2", "0.000000"),
    ("m_eqMidVarGainDcyTime_0", "0.000000"),
    ("m_eqMidVarGainDcyTime_1", "0.000000"),
    ("m_eqMidVarGainDcyTime_2", "0.000000"),
    ("m_eqDynLPEnable", "0"),
    ("m_eqDynLPHoldOff", "0.200000"),
    ("m_eqDynLPInitialFreqHi", "17000.000000"),
    ("m_eqDynLPInitialFreqLo", "10000.000000"),
    ("m_eqDynLPFinalFreqHi", "5000.000000"),
    ("m_eqDynLPFinalFreqLo", "3000.000000"),
    ("m_eqDynLPReleaseFreqHi", "8000.000000"),
    ("m_eqDynLPReleaseFreqLo", "3000.000000"),
    ("m_eqDynLPFinalFreq2Hi", "3000.000000"),
    ("m_eqDynLPFinalFreq2Lo", "2000.000000"),
    ("m_eqTrackingMultFactor", "1.000000"),
    ("m_eqTrackingGain1", "0.000000"),
    ("m_eqTrackingGain2", "0.000000"),
    ("m_eqTrackingGain3", "0.000000"),
    ("m_eqTrackingGain4", "0.000000"),
    ("m_eqTrackingGain5", "0.000000"),
    ("m_eqTrackingGain6", "0.000000"),
    ("m_eqTrackingGain7", "0.000000"),
    ("m_eqTrackingGain8", "0.000000"),
    ("m_eqTrackingGain9", "0.000000"),
    ("m_eqTrackingGain10", "0.000000"),
    ("m_eqTrackingEnable", "0"),
    ("m_eqTrackingBand1Enable", "0"),
    ("m_eqTrackingBand2Enable", "0"),
    ("m_eqTrackingBand3Enable", "0"),
    ("m_eqTrackingBand4Enable", "0"),
    ("m_eqTrackingBand5Enable", "0"),
    ("m_eqTrackingBand6Enable", "0"),
    ("m_eqTrackingBand7Enable", "0"),
    ("m_eqTrackingBand8Enable", "0"),
    ("m_eqTrackingBand9Enable", "0"),
    ("m_eqTrackingBand10Enable", "0"),
];

/// A `map_stroke`'s fields between its global id and its date.
const MAP_STROKE_DEFAULTS: &[(&str, &str)] = &[
    ("m_isEnabled", "1"),
    ("m_gain", "1.000000"),
    ("m_detune", "0"),
    ("m_relStrength", "1"),
    ("m_relStrengthEdited", "0"),
    ("m_relStrengthTop", "16384"),
    ("m_relStrengthTopEdited", "0"),
    ("m_velocityMin", "0"),
    ("m_velocityMax", "127"),
    ("m_attackLen", "0.000000"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn three_zones() -> Project {
        let zone = |path: &str, root_key| NewZone {
            path: path.into(),
            sample_rate: 44100,
            frames: 4394,
            root_key,
        };
        Project::new(
            "Three",
            &[
                zone("audio/c4.wav", 60),
                zone("audio/c3.wav", 48),
                zone("audio/c5.wav", 72),
            ],
            1_700_000_000,
        )
        .unwrap()
    }

    #[test]
    fn a_new_project_reads_back_through_the_views() {
        let project = three_zones();
        assert_eq!(project.name().unwrap(), "Three");
        assert_eq!(
            project.file_format_version().unwrap(),
            KNOWN_FILE_FORMAT_VERSION
        );
        assert_eq!(project.created_by().unwrap().0, "nord-format");

        // Files number low to high by root key.
        let files = project.audio_files().unwrap();
        assert_eq!(
            files
                .iter()
                .map(|f| (f.id, f.path.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (1, "audio/c3.wav"),
                (2, "audio/c4.wav"),
                (3, "audio/c5.wav")
            ]
        );

        // Zones sit high to low, ids rising with the root, ranges as the editor lays them.
        let zones = project.zones().unwrap();
        let layout: Vec<_> = zones
            .iter()
            .map(|z| (z.zone_id, z.root_key, z.bottom_note, z.top_note))
            .collect();
        assert_eq!(
            layout,
            vec![(131, 72, 66, 96), (130, 60, 54, 65), (129, 48, 17, 53)]
        );
        assert!(zones.iter().all(|z| z.enabled && z.strokes.len() == 1));
        assert_eq!(zones[0].strokes[0].velocity, (0, 127));

        let strokes = project.strokes().unwrap();
        assert_eq!(
            strokes
                .iter()
                .map(|s| (s.zone_id, s.global_id, s.file_id))
                .collect::<Vec<_>>(),
            vec![(131, 3, 3), (130, 2, 2), (129, 1, 1)]
        );
        assert_eq!(strokes[0].end, 4394.0);
        assert_eq!(strokes[0].loop_start, 2197.0);
        assert_eq!(strokes[0].start_secondary, 549.25);
        assert_eq!(strokes[0].loop_length, 2196.0);
        assert!(!strokes[0].loop_enabled);
    }

    #[test]
    fn a_new_project_round_trips_byte_exactly() {
        let project = three_zones();
        let text = project.render();
        assert!(text.as_bytes().starts_with(MAGIC));
        let back = Project::parse(&text).unwrap();
        assert_eq!(back, project);
        assert_eq!(back.render(), text);
        // The editor's one oddity: an empty value keeps its trailing space.
        assert!(text.contains("      m_buffer = \n"));
    }

    #[test]
    fn setters_edit_in_place() {
        let mut project = three_zones();
        let before = project.render();
        project.set_name("Renamed").unwrap();
        project.set_root_key(130, 61).unwrap();
        project.set_key_range(131, 62, 100).unwrap();
        project.set_audio_path(2, "moved/c4.wav").unwrap();
        let after = project.render();

        assert_eq!(project.name().unwrap(), "Renamed");
        let zones = project.zones().unwrap();
        assert_eq!(zones[1].root_key, 61);
        assert_eq!((zones[0].bottom_note, zones[0].top_note), (62, 100));
        assert_eq!(project.audio_files().unwrap()[1].path, "moved/c4.wav");

        // Five lines moved and nothing else.
        let changed = before
            .lines()
            .zip(after.lines())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(changed, 5);
        assert_eq!(before.lines().count(), after.lines().count());

        assert!(project.set_root_key(200, 60).is_err());
        assert!(project.set_audio_path(9, "x").is_err());
        assert!(project.set_key_range(131, 70, 60).is_err());
    }

    #[test]
    fn the_editor_keeps_a_secondary_start_between_92_and_the_ceiling() {
        assert_eq!(repaired_secondary_start(5_512.5, 44_100.0, None), 5_512.5);
        assert_eq!(repaired_secondary_start(92.0, 44_100.0, None), 92.0);
        assert_eq!(repaired_secondary_start(22_050.0, 44_100.0, None), 22_050.0);
        assert_eq!(
            repaired_secondary_start(5_512.5, 44_100.0, Some(8_000.0)),
            5_512.5
        );
    }

    #[test]
    fn the_editor_repairs_a_secondary_start_to_half_the_ceiling() {
        // D5: a 441-frame zone stating 1 encodes from 110.25.
        assert_eq!(repaired_secondary_start(1.0, 441.0, None), 110.25);
        assert_eq!(repaired_secondary_start(91.0, 44_100.0, None), 11_025.0);
        assert_eq!(repaired_secondary_start(22_051.0, 44_100.0, None), 11_025.0);
        assert_eq!(
            repaired_secondary_start(5_512.5, 44_100.0, Some(1_000.0)),
            500.0
        );
        assert_eq!(repaired_secondary_start(200.0, 300.0, None), 92.0);
        assert_eq!(repaired_secondary_start(f64::NAN, 44_100.0, None), 11_025.0);
    }

    fn stroke(project: &Project, global_id: u32) -> Stroke {
        project
            .strokes()
            .unwrap()
            .into_iter()
            .find(|s| s.global_id == global_id)
            .unwrap()
    }

    fn zone_stroke(project: &Project, global_id: u32) -> ZoneStroke {
        project
            .zones()
            .unwrap()
            .into_iter()
            .flat_map(|z| z.strokes)
            .find(|s| s.global_id == global_id)
            .unwrap()
    }

    #[test]
    fn a_stroke_field_moves_only_its_own_line() {
        let mut project = three_zones();
        let before = project.render();
        for field in [
            StrokeField::LoopEnabled(true),
            StrokeField::LoopStart(1000.0),
            StrokeField::LoopLength(500.0),
            StrokeField::Gain(0.5),
            StrokeField::VelocityMax(100),
        ] {
            project.set_stroke_field(2, field).unwrap();
        }
        let after = project.render();

        let s = stroke(&project, 2);
        assert!(s.loop_enabled);
        assert_eq!((s.loop_start, s.loop_length), (1000.0, 500.0));
        let z = zone_stroke(&project, 2);
        assert_eq!(z.gain, 0.5);
        assert_eq!(z.velocity, (0, 100));

        // Real numbers keep the editor's six decimals.
        assert!(after.contains("m_loopStart = 1000.000000\n"), "{after}");
        assert!(after.contains("m_gain = 0.500000\n"), "{after}");

        let changed = before
            .lines()
            .zip(after.lines())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(changed, 5);
        assert_eq!(before.lines().count(), after.lines().count());
        // Only the addressed stroke moved.
        assert!(!stroke(&project, 1).loop_enabled);
        assert_eq!(zone_stroke(&project, 3).gain, 1.0);
    }

    #[test]
    fn the_loop_group_and_velocity_defaults_round_trip() {
        let mut project = three_zones();
        for field in [
            StrokeField::Start(2.0),
            StrokeField::Stop(4000.0),
            StrokeField::LoopCrossfade(120.0),
            StrokeField::LoopCrossfadeMode(1),
            StrokeField::LoopDecayEnabled(true),
            StrokeField::LoopDecay(3.5),
            StrokeField::LoopDetune(-7),
            StrokeField::ShortLoopEnabled(true),
            StrokeField::ShortLoopLength(64.0),
            StrokeField::ShortLoopCrossfade(4),
            StrokeField::ShortLoopUsesPitch(false),
            StrokeField::VelocityMin(20),
        ] {
            project.set_stroke_field(1, field).unwrap();
        }
        project
            .set_velocity_defaults(VelocityDefaults {
                attack_amount: 64,
                amplitude: 0,
                timbre: 1,
            })
            .unwrap();

        let s = stroke(&project, 1);
        assert_eq!((s.start, s.stop), (2.0, 4000.0));
        assert_eq!((s.loop_crossfade, s.loop_crossfade_mode), (120.0, 1));
        assert!(s.loop_decay_enabled);
        assert_eq!((s.loop_decay, s.loop_detune), (3.5, -7));
        assert!(s.short_loop_enabled && !s.short_loop_uses_pitch);
        assert_eq!((s.short_loop_length, s.short_loop_crossfade), (64.0, 4));
        assert_eq!(zone_stroke(&project, 1).velocity, (20, 127));
        assert_eq!(
            project.velocity_defaults().unwrap(),
            VelocityDefaults {
                attack_amount: 64,
                amplitude: 0,
                timbre: 1,
            }
        );

        // The whole file still parses back to itself.
        let text = project.render();
        assert_eq!(Project::parse(&text).unwrap().render(), text);
    }

    #[test]
    fn missing_velocity_defaults_leave_the_project_unchanged() {
        let mut project = three_zones();
        project
            .attrs_mut()
            .unwrap()
            .entries
            .retain(|entry| !matches!(entry, Entry::Field { key, .. } if key == "m_velTimbre"));
        let before = project.render();

        assert!(project
            .set_velocity_defaults(VelocityDefaults {
                attack_amount: 64,
                amplitude: 0,
                timbre: 1,
            })
            .is_err());
        assert_eq!(project.render(), before);
    }

    #[test]
    fn a_stroke_field_refuses_what_the_file_cannot_hold() {
        let mut project = three_zones();
        let before = project.render();
        for bad in [
            StrokeField::LoopStart(-1.0),
            StrokeField::LoopStart(f64::NAN),
            StrokeField::LoopStart(f64::INFINITY),
            StrokeField::Gain(-0.5),
            StrokeField::LoopDecay(-1.0),
            StrokeField::VelocityMin(200),
            StrokeField::VelocityMax(128),
        ] {
            assert!(project.set_stroke_field(1, bad).is_err(), "{bad:?}");
        }
        assert!(project
            .set_stroke_field(99, StrokeField::LoopEnabled(true))
            .is_err());
        assert!(project
            .set_stroke_field(99, StrokeField::Gain(1.0))
            .is_err());
        assert_eq!(project.render(), before);
    }

    #[test]
    fn a_new_project_refuses_what_the_editor_cannot_lay_out() {
        let zone = |root_key| NewZone {
            path: "a.wav".into(),
            sample_rate: 44100,
            frames: 1,
            root_key,
        };
        assert!(Project::new("x", &[], 0).is_err());
        assert!(Project::new("x", &[zone(60), zone(60)], 0).is_err());
        assert!(Project::new("x", &[zone(10)], 0).is_err());
        assert!(Project::new("x", &[zone(60)], 0).is_ok());
    }

    #[test]
    fn anything_but_a_project_is_refused() {
        assert!(Project::read_from(&mut b"CBIN....".as_slice()).is_err());
        assert!(Project::parse("Other {\n  a = 1\n}\n").is_err());
    }
}
