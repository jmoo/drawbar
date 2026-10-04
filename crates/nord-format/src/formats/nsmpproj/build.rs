//! Building a sample instrument from a Sample Editor project.
//!
//! [`plan`] resolves a [`Project`]'s zones against the WAVs it names, which a
//! [`Source`] supplies, into a [`Plan`], and [`Plan::encode`] writes that plan as an
//! [`nsmp`] instrument. What the encoder cannot reproduce is refused with a
//! [`BuildError`]; what the instrument has nowhere to hold is a [`Warning`].

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use thiserror::Error as ThisError;

use super::{AudioFile, Project, Stroke, Zone, LOWEST_NOTE};
use crate::error::{Error, ParseError};
use crate::formats::nsmp::{self, codec, encode};
use crate::wav::{read_pcm16, Pcm16};

/// Where an [`AudioFile::path`] points.
///
/// `/` and `\` both separate components, and empty and `.` components are dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioPath {
    /// A leading separator, or a first component holding `:` such as `C:`. Held as
    /// stored.
    Absolute(String),
    /// Components from the project's own folder, where `..` climbs one folder.
    Relative(Vec<String>),
}

impl AudioPath {
    /// What a stored `m_fullName` names.
    pub fn parse(stored: &str) -> AudioPath {
        let slashed = stored.replace('\\', "/");
        let first = slashed.split('/').next().unwrap_or_default();
        if slashed.starts_with('/') || first.contains(':') {
            return AudioPath::Absolute(stored.to_string());
        }
        AudioPath::Relative(
            slashed
                .split('/')
                .filter(|part| !matches!(*part, "" | "."))
                .map(String::from)
                .collect(),
        )
    }

    /// The file's components from the root of a bounded tree, given the project's
    /// folder `dir` as components from that root. `None` for an absolute path, for a
    /// `..` that climbs past the root, and for the root itself.
    pub fn within<'a>(&self, dir: impl IntoIterator<Item = &'a str>) -> Option<Vec<String>> {
        let AudioPath::Relative(parts) = self else {
            return None;
        };
        let mut resolved: Vec<String> = dir.into_iter().map(String::from).collect();
        for part in parts {
            match part.as_str() {
                ".." => {
                    resolved.pop()?;
                }
                part => resolved.push(part.to_string()),
            }
        }
        (!resolved.is_empty()).then_some(resolved)
    }

    /// The file on disk for a project in `dir`: an absolute path as stored, a relative
    /// one joined onto `dir` with each `..` left for the file system to resolve.
    pub fn on_disk(&self, dir: &Path) -> PathBuf {
        match self {
            AudioPath::Absolute(stored) => PathBuf::from(stored),
            AudioPath::Relative(parts) => parts
                .iter()
                .fold(dir.to_path_buf(), |path, part| path.join(part)),
        }
    }
}

/// Supplies the WAVs a project's audio files name.
pub trait Source {
    /// The bytes of `file`'s WAV, found from its [`AudioFile::path`].
    fn wav(&self, file: &AudioFile) -> Result<Cow<'_, [u8]>, Unavailable>;

    /// How a [`BuildError`] and [`PlannedZone::source`] name `file`: its stored path,
    /// unless the source names it otherwise.
    fn name(&self, file: &AudioFile) -> String {
        file.path.clone()
    }
}

/// Why a [`Source`] could not supply a WAV.
#[derive(ThisError, Debug)]
pub enum Unavailable {
    #[error("no such file")]
    Missing,
    /// The path is absolute, or climbs out of the tree the source reads.
    #[error("outside the tree the project's audio is read from")]
    Outside,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Why a WAV is not audio the sample encoder takes.
#[derive(ThisError, Debug)]
pub enum AudioError {
    #[error(transparent)]
    Wav(#[from] Error),
    #[error("{channels} channels, but a stroke holds one or two")]
    Channels { channels: u16 },
    #[error(
        "{rate} Hz, but the encoder needs {needs} Hz because the instrument's resampler \
         is not decoded; resample the WAV first",
        needs = codec::SOURCE_RATE
    )]
    Rate { rate: u32 },
}

/// One WAV as the sample encoder takes it: 16-bit PCM, mono or stereo, at
/// [`codec::SOURCE_RATE`]. It is not resampled: the field lattice is defined against
/// that rate.
///
/// ⚠️ A stereo file becomes a stereo stroke, with both channels under one header.
/// No generation has a stroke that holds more than two channels.
pub fn source_pcm(wav: &[u8]) -> Result<Pcm16, AudioError> {
    let pcm = read_pcm16(wav)?;
    if pcm.channels != 1 && pcm.channels != 2 {
        return Err(AudioError::Channels {
            channels: pcm.channels,
        });
    }
    if pcm.rate != codec::SOURCE_RATE {
        return Err(AudioError::Rate { rate: pcm.rate });
    }
    Ok(pcm)
}

/// A frame position a project states for a zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Start,
    Stop,
    LoopStart,
    LoopEnd,
    LoopCrossfade,
    ShortLoopCrossfade,
}

impl std::fmt::Display for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Position::Start => "start",
            Position::Stop => "stop",
            Position::LoopStart => "loop start",
            Position::LoopEnd => "loop end",
            Position::LoopCrossfade => "loop crossfade",
            Position::ShortLoopCrossfade => "short loop crossfade",
        })
    }
}

/// Why a project cannot be built. A `zone` is 1-based, in the project's order: highest
/// first.
#[derive(ThisError, Debug)]
#[non_exhaustive]
pub enum BuildError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    /// The editor bakes enabled EQ into the audio. Each stage is named as
    /// [`Project::active_eq`] names it.
    #[error(
        "the project enables {}, whose effect the editor bakes into the audio; disable it \
         before building because this encoder cannot reproduce that processing",
        .0.join(", ")
    )]
    ActiveEq(Vec<String>),
    /// A v2 preset velocity level the narrow `sty` has no decoded value for.
    #[error("{field} = {value} has no decoded v2 preset level")]
    VelocityLevel { field: &'static str, value: u8 },
    #[error("zone{zone}'s root note {root} is outside its range {bottom}..={top}")]
    RootOutsideRange {
        zone: usize,
        root: u8,
        bottom: u8,
        top: u8,
    },
    /// The zone below this one reaches the highest note.
    #[error("zone{} reaches note {top}, leaving no range for zone{zone}", .zone + 1)]
    NoRangeLeft { zone: usize, top: u8 },
    /// The encoded keyboard map tiles its zones, so each zone starts one note above
    /// the next one down, and the lowest at [`LOWEST_NOTE`].
    #[error(
        "zone{zone} starts at note {bottom}, but its encoded range would start at \
         {encoded}; the encoded keyboard map tiles its zones, so that gap or overlap \
         cannot be reproduced"
    )]
    KeyRangeGap {
        zone: usize,
        bottom: u8,
        encoded: u8,
    },
    #[error("zone{zone} is disabled in the project. Enable or remove it before building.")]
    ZoneDisabled { zone: usize },
    /// A velocity split or a round robin.
    #[error(
        "zone{zone} plays {strokes} strokes, which is a velocity split or a round robin; \
         this writer supports one stroke per zone"
    )]
    StrokeCount { zone: usize, strokes: usize },
    #[error("zone{zone}'s only stroke is switched off")]
    StrokeOff { zone: usize },
    #[error(
        "zone{zone} sets detune {detune} and velocity {}..={} on its stroke; where the \
         instrument applies those is not decoded, so nothing here reproduces them",
        .velocity.0, .velocity.1
    )]
    StrokeExpression {
        zone: usize,
        detune: i32,
        velocity: (u8, u8),
    },
    #[error("zone{zone} names stroke {stroke}, which the project does not hold")]
    NoStroke { zone: usize, stroke: u32 },
    #[error("zone{zone} plays audio file {file}, which the project does not hold")]
    NoAudioFile { zone: usize, file: u32 },
    /// `path` is the file as the [`Source`] names it.
    #[error("{path}: {why}")]
    Unavailable { path: String, why: Unavailable },
    /// `path` is the file as the [`Source`] names it.
    #[error("{path}: {why}")]
    Audio { path: String, why: AudioError },
    #[error("zone{zone} plays frames {start}..{stop} of {path}, which is nothing")]
    EmptyTrim {
        zone: usize,
        start: usize,
        stop: usize,
        path: String,
    },
    #[error(
        "zone{zone}'s {position} is at frame {value}, outside the {frames} frames its \
         audio holds"
    )]
    FrameOutside {
        zone: usize,
        position: Position,
        value: f64,
        frames: usize,
    },
    /// `field` is the project's `m_loopLengthLong` or `m_loopLengthShort`.
    #[error("zone{zone}'s {field} is {length}, which is not a loop")]
    LoopLength {
        zone: usize,
        field: &'static str,
        length: f64,
    },
    #[error(
        "zone{zone} loops from frame {loop_start} but its audio is trimmed to start at \
         {start}; the loop would begin before the sample does"
    )]
    LoopBeforeStart {
        zone: usize,
        loop_start: usize,
        start: usize,
    },
    /// Only the long loop's linear fade, mode 0, is decoded.
    #[error(
        "zone{zone} sets m_loopXFModeLong = {mode}; only the linear fade (mode 0) is \
         decoded, and the fade is baked into the audio, so this one cannot be written"
    )]
    CrossfadeMode { zone: usize, mode: u32 },
}

/// A loop setting in the project that no instrument field holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Dropped {
    /// `m_loopDetune`.
    LoopDetune(i32),
    /// `m_loopDecayEnabled` and the `m_loopDecay` amount, which a narrow stroke
    /// header does not carry.
    LoopDecay(f64),
    /// `m_loopDecayEnabled`. A wide stroke header carries the amount, not the switch.
    LoopDecaySwitch,
    /// `m_shortLoopUsesPitch = 0`.
    ShortLoopIgnoresPitch,
    /// `m_loopXFadeLengthLong` while the short loop is the one encoded.
    LongLoopCrossfade(f64),
    /// `m_loopXFModeLong` while the short loop is the one encoded.
    LongLoopCrossfadeMode(u32),
    /// The instrument's own `m_loopDecayEnabled`.
    InstrumentLoopDecay,
}

impl std::fmt::Display for Dropped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dropped::LoopDetune(cents) => write!(f, "m_loopDetune = {cents}"),
            Dropped::LoopDecay(amount) => {
                write!(f, "m_loopDecayEnabled and m_loopDecay = {amount}")
            }
            Dropped::LoopDecaySwitch => {
                f.write_str("m_loopDecayEnabled (the amount is written, the switch is not)")
            }
            Dropped::ShortLoopIgnoresPitch => f.write_str("m_shortLoopUsesPitch = 0"),
            Dropped::LongLoopCrossfade(frames) => write!(
                f,
                "m_loopXFadeLengthLong = {frames} (the short loop is the one encoded)"
            ),
            Dropped::LongLoopCrossfadeMode(mode) => write!(f, "m_loopXFModeLong = {mode}"),
            Dropped::InstrumentLoopDecay => f.write_str("the instrument's own m_loopDecayEnabled"),
        }
    }
}

/// What a build writes, but not as the project states it. A `zone` is 1-based, as in
/// [`BuildError`].
#[derive(Debug, Clone, PartialEq)]
pub enum Warning {
    /// The map's gain is outside what the instrument holds, so it is clamped at
    /// [`encode::MAX_MAP_GAIN_DB`].
    MapGainClamped {
        gain: f64,
    },
    /// A zone gain at or above [`WRAPPING_ZONE_GAIN`].
    GainWraps {
        zone: usize,
        gain: f64,
    },
    Dropped {
        zone: usize,
        settings: Vec<Dropped>,
    },
    /// The editor replaces this `m_startSecondary` on load, and the build encodes from
    /// the replacement, a file frame.
    SecondaryStartRepaired {
        zone: usize,
        stated: f64,
        encoded: f64,
    },
}

impl Warning {
    /// The 1-based zone the warning concerns, or `None` for the whole instrument.
    pub fn zone(&self) -> Option<usize> {
        match self {
            Warning::MapGainClamped { .. } => None,
            Warning::GainWraps { zone, .. }
            | Warning::Dropped { zone, .. }
            | Warning::SecondaryStartRepaired { zone, .. } => Some(*zone),
        }
    }
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Warning::MapGainClamped { gain } => write!(
                f,
                "the map's gain is {gain}, which the instrument clamps at +{:.3} dB, as the \
                 editor does",
                encode::MAX_MAP_GAIN_DB
            ),
            Warning::GainWraps { zone, gain } => write!(
                f,
                "zone{zone} sets gain {gain}, which overflows both of the instrument's gain \
                 fields; the file will state a far quieter level, as the editor's render of \
                 this project does"
            ),
            Warning::Dropped { zone, settings } => {
                let settings: Vec<String> = settings.iter().map(Dropped::to_string).collect();
                write!(
                    f,
                    "zone{zone} sets {}, which the instrument has nowhere to hold",
                    settings.join(", ")
                )
            }
            Warning::SecondaryStartRepaired {
                zone,
                stated,
                encoded,
            } => write!(
                f,
                "zone{zone} states m_startSecondary = {stated}, which the editor repairs on \
                 load; encoded from frame {encoded} as the editor would"
            ),
        }
    }
}

/// The zone gain at which both of the instrument's gain fields overflow their 24 bits.
/// The file still matches the editor's render, but no longer states the project's gain.
pub const WRAPPING_ZONE_GAIN: f64 = 16.0;

/// One zone of a project, resolved: the audio region it plays and where on the
/// keyboard it plays it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedZone {
    pub global_id: u32,
    pub root_key: u8,
    pub top_note: u8,
    /// Interleaved, so `samples.len()` is `channels` times the frame count.
    pub samples: Vec<i16>,
    pub channels: u16,
    /// The file frame `samples` starts at: the project's `m_start`.
    pub start: usize,
    /// The audio file, as the [`Source`] names it.
    pub source: String,
    pub loops: Option<encode::Loop>,
    /// Where the stream resynchronizes, in frames from the start of `samples`.
    pub secondary_start: f64,
    /// The project's `m_startSecondary` when the editor would not keep it.
    pub repaired_secondary_start: Option<f64>,
    /// The zone's playing gain as a linear ratio.
    pub gain: f64,
    /// The stroke's `m_loopDecay`, which only a wide header carries.
    pub loop_decay: f32,
    /// Loop settings in the project that the instrument has no field for.
    pub dropped: Vec<Dropped>,
}

/// A project resolved into everything its instrument states, ready to encode.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The instrument name the project gives.
    pub name: String,
    /// The generation the plan was resolved for.
    pub layout: codec::Layout,
    pub preset: encode::Preset,
    /// `map_info.m_gain`, a linear ratio.
    pub map_gain: f64,
    /// Highest first, as the project orders them.
    pub zones: Vec<PlannedZone>,
}

impl Plan {
    /// What the instrument will state differently from the project: the map's gain
    /// first, then each zone's warnings in zone order.
    pub fn warnings(&self) -> Vec<Warning> {
        let mut warnings = Vec::new();
        let ceiling = 10f64.powf(encode::MAX_MAP_GAIN_DB / 20.0);
        if !(0.0..=ceiling).contains(&self.map_gain) {
            warnings.push(Warning::MapGainClamped {
                gain: self.map_gain,
            });
        }
        for (index, zone) in self.zones.iter().enumerate() {
            let number = index + 1;
            if zone.gain >= WRAPPING_ZONE_GAIN {
                warnings.push(Warning::GainWraps {
                    zone: number,
                    gain: zone.gain,
                });
            }
            if !zone.dropped.is_empty() {
                warnings.push(Warning::Dropped {
                    zone: number,
                    settings: zone.dropped.clone(),
                });
            }
            if let Some(stated) = zone.repaired_secondary_start {
                warnings.push(Warning::SecondaryStartRepaired {
                    zone: number,
                    stated,
                    encoded: zone.secondary_start + zone.start as f64,
                });
            }
        }
        warnings
    }

    /// The instrument, named `name` and coded with `predictor`. `Some(shift)` quantizes
    /// every stroke at that shift instead of the shift rule's; see
    /// [`encode::Options::shift`]. Refuses what [`encode::multi_zone`] refuses.
    pub fn encode(
        &self,
        name: &str,
        predictor: encode::Predictor,
        shift: Option<u8>,
    ) -> Result<crate::Sample, Error> {
        let zones: Vec<encode::NewZone> = self
            .zones
            .iter()
            .map(|z| encode::NewZone {
                source: &z.samples,
                channels: z.channels,
                root_key: z.root_key,
                top_note: z.top_note,
                global_id: z.global_id,
                loops: z.loops,
                secondary_start: z.secondary_start,
                shift,
                gain: z.gain,
                loop_decay: z.loop_decay,
            })
            .collect();
        encode::multi_zone(
            encode::Instrument {
                name,
                map_gain: self.map_gain,
                predictor,
                layout: self.layout,
                preset: self.preset,
            },
            &zones,
        )
    }
}

/// Resolve `project` for the generation `layout`, reading its WAVs from `source`.
///
/// Anything the editor can express that the encoder cannot lay out is refused by
/// name, because dropping it would silently change what the project describes.
pub fn plan<S: Source + ?Sized>(
    project: &Project,
    layout: codec::Layout,
    source: &S,
) -> Result<Plan, BuildError> {
    let active_eq = project.active_eq()?;
    if !active_eq.is_empty() {
        return Err(BuildError::ActiveEq(active_eq));
    }
    let preset = preset(project, layout)?;
    let zones = zones(project, layout, source)?;
    Ok(Plan {
        name: project.name()?,
        layout,
        preset,
        map_gain: project.map_gain()?,
        zones,
    })
}

/// The part of a project's preset each generation can represent.
fn preset(project: &Project, layout: codec::Layout) -> Result<encode::Preset, BuildError> {
    let mut preset = encode::Preset {
        dynamics_enabled: project.dynamics_enabled()?,
        ..encode::Preset::default()
    };
    if layout == codec::Layout::V2 {
        let defaults = project.velocity_defaults()?;
        let level = |field, value| {
            nsmp::velocity_level(value).ok_or(BuildError::VelocityLevel { field, value })
        };
        preset.velocity_to_amplitude = level("m_velAmpl", defaults.amplitude)?;
        preset.velocity_to_timbre = level("m_velTimbre", defaults.timbre)?;
    }
    Ok(preset)
}

fn zones<S: Source + ?Sized>(
    project: &Project,
    layout: codec::Layout,
    source: &S,
) -> Result<Vec<PlannedZone>, BuildError> {
    let files = project.audio_files()?;
    let strokes = project.strokes()?;
    let zones = project.zones()?;
    let instrument_decay = project.loop_decay_enabled()?;
    validate_key_ranges(&zones)?;

    zones
        .iter()
        .enumerate()
        .map(|(index, zone)| {
            let at = index + 1;
            if !zone.enabled {
                return Err(BuildError::ZoneDisabled { zone: at });
            }
            let [layer] = zone.strokes.as_slice() else {
                return Err(BuildError::StrokeCount {
                    zone: at,
                    strokes: zone.strokes.len(),
                });
            };
            if !layer.enabled {
                return Err(BuildError::StrokeOff { zone: at });
            }
            if layer.detune != 0 || layer.velocity != (0, 127) {
                return Err(BuildError::StrokeExpression {
                    zone: at,
                    detune: layer.detune,
                    velocity: layer.velocity,
                });
            }
            let stroke = strokes
                .iter()
                .find(|s| s.global_id == layer.global_id)
                .ok_or(BuildError::NoStroke {
                    zone: at,
                    stroke: layer.global_id,
                })?;
            let file =
                files
                    .iter()
                    .find(|f| f.id == stroke.file_id)
                    .ok_or(BuildError::NoAudioFile {
                        zone: at,
                        file: stroke.file_id,
                    })?;

            let path = source.name(file);
            let wav = source.wav(file).map_err(|why| BuildError::Unavailable {
                path: path.clone(),
                why,
            })?;
            let audio = source_pcm(&wav).map_err(|why| BuildError::Audio {
                path: path.clone(),
                why,
            })?;
            let frames = audio.frames();
            let channels = usize::from(audio.channels);
            let start = frame(at, Position::Start, stroke.start, frames)?;
            let stop = frame(at, Position::Stop, stroke.stop, frames)?;
            if start >= stop {
                return Err(BuildError::EmptyTrim {
                    zone: at,
                    start,
                    stop,
                    path,
                });
            }
            let (loops, mut dropped) = zone_loop(at, stroke, start, stop, layout)?;
            if loops.is_some() && instrument_decay {
                dropped.push(Dropped::InstrumentLoopDecay);
            }
            let encoded_secondary = stroke.encoded_secondary_start();
            Ok(PlannedZone {
                global_id: layer.global_id,
                root_key: zone.root_key,
                top_note: zone.top_note,
                channels: audio.channels,
                samples: audio.samples[start * channels..stop * channels].to_vec(),
                source: path,
                loops,
                start,
                secondary_start: encoded_secondary - start as f64,
                repaired_secondary_start: (encoded_secondary != stroke.start_secondary)
                    .then_some(stroke.start_secondary),
                gain: layer.gain,
                loop_decay: stroke.loop_decay as f32,
                dropped,
            })
        })
        .collect()
}

/// One stroke's loop as the encoder states it, and the loop settings that reach no
/// instrument.
///
/// The project's loop points count frames of the audio file, so they move with the
/// trim. Whichever loop is switched on becomes the container's single loop: a short
/// loop has the same start with the short length, and nothing in the file records which
/// of the two it was. The long loop states its crossfade in frames, and the short loop
/// as a percentage of its length. Only the switched-on loop's fade reaches the audio.
///
/// Inferred from specimens; not confirmed on hardware.
fn zone_loop(
    at: usize,
    stroke: &Stroke,
    start: usize,
    stop: usize,
    layout: codec::Layout,
) -> Result<(Option<encode::Loop>, Vec<Dropped>), BuildError> {
    if !stroke.loop_enabled {
        return Ok((None, Vec::new()));
    }
    let short = stroke.short_loop_enabled;
    let (length, field) = if short {
        (stroke.short_loop_length, "m_loopLengthShort")
    } else {
        (stroke.loop_length, "m_loopLengthLong")
    };
    let stated = stroke.encoded_loop_start();
    let loop_start = frame(at, Position::LoopStart, stated, stop)?;
    if !length.is_finite() || length <= 0.0 {
        return Err(BuildError::LoopLength {
            zone: at,
            field,
            length,
        });
    }
    let end = frame(at, Position::LoopEnd, stated + length, stop)?;
    if loop_start < start {
        return Err(BuildError::LoopBeforeStart {
            zone: at,
            loop_start,
            start,
        });
    }

    // Mode 1 rewrites the long loop's tail in a way not yet decoded. It affects only
    // that fade: with the short loop switched on, the editor writes the same bytes
    // whatever the mode.
    // Inferred from specimens; not confirmed on hardware.
    if !short && stroke.loop_crossfade_mode != 0 {
        return Err(BuildError::CrossfadeMode {
            zone: at,
            mode: stroke.loop_crossfade_mode,
        });
    }
    let crossfade = if short {
        exact_frame(
            at,
            Position::ShortLoopCrossfade,
            f64::from(stroke.short_loop_crossfade) / 100.0 * length,
            stop,
        )?
    } else {
        exact_frame(at, Position::LoopCrossfade, stroke.loop_crossfade, stop)?
    };

    // These reach no instrument: the editor writes the same bytes whatever they hold.
    let mut dropped = Vec::new();
    if stroke.loop_detune != 0 {
        dropped.push(Dropped::LoopDetune(stroke.loop_detune));
    }
    if stroke.loop_decay_enabled {
        dropped.push(match layout {
            codec::Layout::V2 => Dropped::LoopDecay(stroke.loop_decay),
            codec::Layout::V3 | codec::Layout::V4 => Dropped::LoopDecaySwitch,
        });
    }
    if short && !stroke.short_loop_uses_pitch {
        dropped.push(Dropped::ShortLoopIgnoresPitch);
    }
    if short && stroke.loop_crossfade != 0.0 {
        dropped.push(Dropped::LongLoopCrossfade(stroke.loop_crossfade));
    }
    if short && stroke.loop_crossfade_mode != 0 {
        dropped.push(Dropped::LongLoopCrossfadeMode(stroke.loop_crossfade_mode));
    }
    Ok((
        Some(encode::Loop::new(loop_start - start, end - start).crossfade(crossfade)),
        dropped,
    ))
}

fn validate_key_ranges(zones: &[Zone]) -> Result<(), BuildError> {
    for (index, zone) in zones.iter().enumerate() {
        let at = index + 1;
        if !(zone.bottom_note..=zone.top_note).contains(&zone.root_key) {
            return Err(BuildError::RootOutsideRange {
                zone: at,
                root: zone.root_key,
                bottom: zone.bottom_note,
                top: zone.top_note,
            });
        }
        let encoded = match zones.get(index + 1) {
            Some(below) => below
                .top_note
                .checked_add(1)
                .ok_or(BuildError::NoRangeLeft {
                    zone: at,
                    top: below.top_note,
                })?,
            None => LOWEST_NOTE,
        };
        if zone.bottom_note != encoded {
            return Err(BuildError::KeyRangeGap {
                zone: at,
                bottom: zone.bottom_note,
                encoded,
            });
        }
    }
    Ok(())
}

/// A project's frame position as an index into the file it points at.
///
/// Positions are `%f` decimals counted at 44,100 Hz, whatever the file's own rate.
/// A zone encodes `start..stop`, not the whole `begin..end` extent.
/// Inferred from specimens; not confirmed on hardware.
fn frame(at: usize, position: Position, value: f64, frames: usize) -> Result<usize, BuildError> {
    Ok(exact_frame(at, position, value, frames)?.round() as usize)
}

/// [`frame`] without the rounding, for a value the encoder needs exact: a crossfade
/// stated as a percentage can fall between two frames, and rounding it first would move
/// a field on the field lattice.
fn exact_frame(
    at: usize,
    position: Position,
    value: f64,
    frames: usize,
) -> Result<f64, BuildError> {
    if !value.is_finite() || !(0.0..=frames as f64).contains(&value) {
        return Err(BuildError::FrameOutside {
            zone: at,
            position,
            value,
            frames,
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::nsmpproj::NewZone;

    #[test]
    fn a_relative_path_resolves_from_the_projects_folder() {
        let within = |stored: &str| AudioPath::parse(stored).within(["Marimba"]);
        let path = |parts: &[&str]| Some(parts.iter().map(|p| p.to_string()).collect());
        assert_eq!(
            within("audio/c4.wav"),
            path(&["Marimba", "audio", "c4.wav"])
        );
        assert_eq!(
            within(r"audio\c4.wav"),
            path(&["Marimba", "audio", "c4.wav"])
        );
        assert_eq!(within("./c4.wav"), path(&["Marimba", "c4.wav"]));
        assert_eq!(within("../Shared/c4.wav"), path(&["Shared", "c4.wav"]));
        assert_eq!(AudioPath::parse("c4.wav").within([]), path(&["c4.wav"]));
    }

    #[test]
    fn a_path_outside_a_bounded_tree_resolves_to_nothing() {
        let within = |stored: &str| AudioPath::parse(stored).within(["Marimba"]);
        assert_eq!(within("../../c4.wav"), None);
        assert_eq!(within(".."), None);
        assert_eq!(within("/abs/c4.wav"), None);
        assert_eq!(within("C:/x.wav"), None);
        assert_eq!(within(r"C:\Samples\c4.wav"), None);
        assert_eq!(AudioPath::parse("").within([]), None);
    }

    #[test]
    fn an_empty_path_names_the_projects_own_folder() {
        assert_eq!(AudioPath::parse(""), AudioPath::Relative(Vec::new()));
        assert_eq!(
            AudioPath::parse("").within(["Marimba"]),
            Some(vec!["Marimba".to_string()])
        );
    }

    #[test]
    fn on_disk_an_absolute_path_is_used_as_written_and_a_relative_one_joins_the_folder() {
        let dir = Path::new("kit");
        let on_disk = |stored: &str| AudioPath::parse(stored).on_disk(dir);
        assert_eq!(on_disk("/abs/c4.wav"), Path::new("/abs/c4.wav"));
        assert_eq!(on_disk("C:/x.wav"), Path::new("C:/x.wav"));
        assert_eq!(on_disk(r"audio\c4.wav"), Path::new("kit/audio/c4.wav"));
        assert_eq!(on_disk("./c4.wav"), Path::new("kit/c4.wav"));
        assert_eq!(on_disk("../../c4.wav"), Path::new("kit/../../c4.wav"));
    }

    /// WAVs by stored path.
    struct Memory(Vec<(&'static str, Vec<u8>)>);

    impl Source for Memory {
        fn wav(&self, file: &AudioFile) -> Result<Cow<'_, [u8]>, Unavailable> {
            self.0
                .iter()
                .find(|(path, _)| *path == file.path)
                .map(|(_, wav)| Cow::Borrowed(wav.as_slice()))
                .ok_or(Unavailable::Missing)
        }
    }

    const FRAMES: usize = 8192;

    fn wav(rate: u32, channels: u16) -> Vec<u8> {
        let samples: Vec<i16> = (0..FRAMES * usize::from(channels))
            .map(|k| (k % 512) as i16 * 16 - 4096)
            .collect();
        crate::wav::pcm16(&samples, rate, channels).unwrap()
    }

    fn two_zones() -> Project {
        let zone = |path: &str, root_key| NewZone {
            path: path.into(),
            sample_rate: codec::SOURCE_RATE,
            frames: FRAMES as u64,
            root_key,
        };
        Project::new("Marimba", &[zone("low.wav", 48), zone("high.wav", 72)], 0).unwrap()
    }

    fn both(low: Vec<u8>, high: Vec<u8>) -> Memory {
        Memory(vec![("low.wav", low), ("high.wav", high)])
    }

    #[test]
    fn a_two_zone_project_builds_with_its_roots_and_top_notes() {
        let project = two_zones();
        let source = both(wav(codec::SOURCE_RATE, 1), wav(codec::SOURCE_RATE, 2));
        let plan = plan(&project, codec::Layout::V2, &source).unwrap();
        assert_eq!(plan.name, "Marimba");
        let sources: Vec<&str> = plan.zones.iter().map(|z| z.source.as_str()).collect();
        assert_eq!(sources, ["high.wav", "low.wav"], "highest zone first");
        assert_eq!(plan.zones[0].channels, 2);
        assert_eq!(plan.zones[1].samples.len(), FRAMES - plan.zones[1].start);
        assert!(plan.warnings().is_empty(), "{:?}", plan.warnings());

        let sample = plan
            .encode(&plan.name, encode::Predictor::Minimizing, None)
            .unwrap();
        let built: Vec<(u8, u8)> = sample
            .zones()
            .unwrap()
            .iter()
            .map(|z| (z.root_key, z.top_note))
            .collect();
        let stated: Vec<(u8, u8)> = project
            .zones()
            .unwrap()
            .iter()
            .map(|z| (z.root_key, z.top_note))
            .collect();
        assert_eq!(built, stated);
    }

    #[test]
    fn a_missing_wav_is_named_by_its_stored_path() {
        let source = Memory(vec![("high.wav", wav(codec::SOURCE_RATE, 1))]);
        let refused = plan(&two_zones(), codec::Layout::V2, &source).unwrap_err();
        assert!(
            matches!(
                &refused,
                BuildError::Unavailable { path, why: Unavailable::Missing } if path == "low.wav"
            ),
            "{refused:?}"
        );
    }

    #[test]
    fn a_wav_at_another_rate_is_refused_by_name() {
        let source = both(wav(codec::SOURCE_RATE, 1), wav(48_000, 1));
        let refused = plan(&two_zones(), codec::Layout::V2, &source).unwrap_err();
        assert!(
            matches!(
                &refused,
                BuildError::Audio { path, why: AudioError::Rate { rate: 48_000 } }
                    if path == "high.wav"
            ),
            "{refused:?}"
        );
        assert!(
            refused.to_string().starts_with("high.wav: 48000 Hz"),
            "{refused}"
        );
    }

    #[test]
    fn a_wav_with_more_than_two_channels_is_refused_by_name() {
        let source = both(wav(codec::SOURCE_RATE, 4), wav(codec::SOURCE_RATE, 1));
        let refused = plan(&two_zones(), codec::Layout::V2, &source).unwrap_err();
        assert!(
            matches!(
                &refused,
                BuildError::Audio { path, why: AudioError::Channels { channels: 4 } }
                    if path == "low.wav"
            ),
            "{refused:?}"
        );
    }

    #[test]
    fn enabled_eq_is_refused_because_the_editor_bakes_it_into_the_audio() {
        let mut project = two_zones();
        project
            .root
            .require_mut("instrument")
            .unwrap()
            .set_field("m_eqLowCutEnable", "1")
            .unwrap();
        let source = both(wav(codec::SOURCE_RATE, 1), wav(codec::SOURCE_RATE, 1));
        let refused = plan(&project, codec::Layout::V2, &source).unwrap_err();
        assert!(
            matches!(&refused, BuildError::ActiveEq(stages) if stages == &["instrument.m_eqLowCutEnable"]),
            "{refused:?}"
        );
    }

    fn map_zone(root_key: u8, bottom_note: u8, top_note: u8) -> Zone {
        Zone {
            zone_id: 0,
            root_key,
            enabled: true,
            bottom_note,
            top_note,
            strokes: Vec::new(),
        }
    }

    #[test]
    fn a_project_key_map_must_be_representable_by_top_notes() {
        let valid = [map_zone(72, 61, 84), map_zone(48, LOWEST_NOTE, 60)];
        assert!(validate_key_ranges(&valid).is_ok());

        let gap = [map_zone(72, 62, 84), map_zone(48, LOWEST_NOTE, 60)];
        assert!(matches!(
            validate_key_ranges(&gap),
            Err(BuildError::KeyRangeGap {
                zone: 1,
                bottom: 62,
                encoded: 61
            })
        ));

        let misplaced_root = [map_zone(60, 61, 84), map_zone(48, LOWEST_NOTE, 60)];
        assert!(matches!(
            validate_key_ranges(&misplaced_root),
            Err(BuildError::RootOutsideRange { zone: 1, .. })
        ));

        let raised_floor = [map_zone(60, LOWEST_NOTE + 1, 84)];
        assert!(matches!(
            validate_key_ranges(&raised_floor),
            Err(BuildError::KeyRangeGap { zone: 1, .. })
        ));

        let top = u8::MAX;
        let no_room = [map_zone(top, top, top), map_zone(48, LOWEST_NOTE, top)];
        assert!(matches!(
            validate_key_ranges(&no_room),
            Err(BuildError::NoRangeLeft {
                zone: 1,
                top: u8::MAX
            })
        ));
    }

    #[test]
    fn a_projects_loop_maps_onto_the_one_the_container_holds() {
        let long = stroke_with(|s| s.loop_enabled = true);
        let (points, dropped) = zone_loop(1, &long, 1_000, 88_200, codec::Layout::V4).unwrap();
        assert_eq!(points, Some(encode::Loop::new(15_384, 31_768)));
        assert!(dropped.is_empty());

        let short = stroke_with(|s| {
            s.loop_enabled = true;
            s.short_loop_enabled = true;
            s.short_loop_length = 1_024.0;
            s.short_loop_crossfade = 25;
            s.loop_crossfade = 4_096.0;
            s.loop_crossfade_mode = 1;
        });
        let (points, dropped) = zone_loop(1, &short, 0, 88_200, codec::Layout::V4).unwrap();
        assert_eq!(
            points,
            Some(encode::Loop::new(16_384, 17_408).crossfade(256.0))
        );
        assert_eq!(
            dropped,
            [
                Dropped::LongLoopCrossfade(4_096.0),
                Dropped::LongLoopCrossfadeMode(1)
            ]
        );

        let unfaded = stroke_with(|s| {
            s.loop_enabled = true;
            s.short_loop_enabled = true;
            s.short_loop_length = 1_024.0;
            s.short_loop_crossfade = 0;
        });
        let (points, _) = zone_loop(1, &unfaded, 0, 88_200, codec::Layout::V4).unwrap();
        assert_eq!(points, Some(encode::Loop::new(16_384, 17_408)));

        let off = stroke_with(|_| {});
        assert_eq!(
            zone_loop(1, &off, 0, 88_200, codec::Layout::V4).unwrap().0,
            None
        );
    }

    #[test]
    fn loop_settings_with_nowhere_to_go_are_named() {
        let refused = |edit: fn(&mut Stroke)| {
            let mut s = stroke_with(|s| s.loop_enabled = true);
            edit(&mut s);
            zone_loop(1, &s, 0, 88_200, codec::Layout::V4).unwrap_err()
        };
        assert!(matches!(
            refused(|s| s.loop_crossfade_mode = 1),
            BuildError::CrossfadeMode { zone: 1, mode: 1 }
        ));
        assert!(matches!(
            refused(|s| s.loop_length = 0.0),
            BuildError::LoopLength {
                field: "m_loopLengthLong",
                ..
            }
        ));
        assert!(matches!(
            refused(|s| s.loop_length = 90_000.0),
            BuildError::FrameOutside {
                position: Position::LoopEnd,
                ..
            }
        ));

        let trimmed = stroke_with(|s| s.loop_enabled = true);
        assert!(matches!(
            zone_loop(1, &trimmed, 20_000, 88_200, codec::Layout::V4),
            Err(BuildError::LoopBeforeStart {
                loop_start: 16_384,
                start: 20_000,
                ..
            })
        ));

        let mut noisy = stroke_with(|s| s.loop_enabled = true);
        noisy.loop_detune = -50;
        noisy.loop_decay_enabled = true;
        let (points, dropped) = zone_loop(1, &noisy, 0, 88_200, codec::Layout::V4).unwrap();
        assert!(points.is_some());
        assert_eq!(
            dropped,
            [Dropped::LoopDetune(-50), Dropped::LoopDecaySwitch]
        );

        let (_, narrow) = zone_loop(1, &noisy, 0, 88_200, codec::Layout::V2).unwrap();
        assert_eq!(narrow, [Dropped::LoopDetune(-50), Dropped::LoopDecay(20.0)]);
    }

    /// The canonical LP rung, whose loop the editor stores at 16384..32768.
    fn stroke_with(edit: impl FnOnce(&mut Stroke)) -> Stroke {
        let mut stroke = Stroke {
            zone_id: 129,
            global_id: 1,
            file_id: 1,
            begin: 0.0,
            end: 88_200.0,
            start: 0.0,
            start_secondary: 11_025.0,
            stop: 88_200.0,
            loop_enabled: false,
            short_loop_enabled: false,
            loop_start: 16_384.0,
            loop_length: 16_384.0,
            short_loop_length: 0.0,
            loop_crossfade: 0.0,
            loop_crossfade_mode: 0,
            short_loop_crossfade: 10,
            short_loop_uses_pitch: true,
            loop_detune: 0,
            loop_decay_enabled: false,
            loop_decay: 20.0,
        };
        edit(&mut stroke);
        stroke
    }

    #[test]
    fn a_frame_position_is_checked_before_rounding() {
        assert_eq!(frame(1, Position::Start, 0.4, 10).unwrap(), 0);
        for value in [-0.4, 10.4, f64::NAN, f64::INFINITY] {
            assert!(frame(1, Position::Start, value, 10).is_err(), "{value}");
        }
    }
}
