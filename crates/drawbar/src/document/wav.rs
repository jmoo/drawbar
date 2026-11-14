//! The WAV document: the whole file's waveform, audition through the app's player, and a
//! gain edit.
//!
//! A WAV reads at any encoding a take arrives in (integer PCM of 8 to 32 bits, or float),
//! and is drawn and played as float. A gain rewrites the samples of the data chunk in the
//! file's own encoding and nothing else, so every other chunk the file holds survives it;
//! rounding to 16 bits happens only when a library is built.

use std::ops::{Range, RangeInclusive};

use eframe::egui;
use nord_format::take::detect::{self, Envelope};
use nord_format::wav::{self, Audio, Encoding, Layout};

use super::controls::Sets;
use super::header::{Body, Cell};
use super::{adding, audition, hit, sample, take};
use crate::app;
use crate::browser::Kind;
use crate::icon::Glyph;
use crate::workspace::{Bytes, LocalEntity};
use nord_format::formats::npno::{encode, Bank};

/// The extension a WAV carries, and the tag it is kept under.
pub const EXTENSION: &str = "wav";

/// The path of a WAV's gain: its total in dB against the file as last saved.
pub const GAIN: &str = "gain";

/// The start of the path of a moved cut, `cut/060-b0-v22/attack-end`: the hit by its
/// attack's label, then the cut. Its value is the frame the cut moves to.
pub const CUT: &str = "cut/";

/// The path prefix of a set that adds the region it names, over the frames its value
/// gives as `start..end`.
pub const ADDED: &str = "add/";

/// The path prefix of a set that adds a loop, over the frames its value gives as
/// `start..end`, to the attack region it names.
pub const LOOPED: &str = "loop/";

/// The path prefix of a set that deletes, from the sample whose attack it names, the
/// piece after it: `note`, `loop` or `key-off`.
pub const DELETED: &str = "delete/";

/// The path prefix of a set that renames the sample whose attack it names to the sample
/// whose attack label its value is, every region of it.
pub const RENAMED: &str = "rename/";

/// The path of a set that replaces every region and loop the take marks with what
/// Detect found. Its value holds a line per region, `<start> <frames> <label>`, and a
/// line per loop, `loop <start> <end>`, its end its last frame.
pub const DETECTED: &str = "detect";

/// The value of a [`DETECTED`] set marking `marks`, whose loops are forward.
pub(crate) fn detected_value(marks: &wav::Marks) -> String {
    let regions = marks
        .regions
        .iter()
        .map(|region| format!("{} {} {}", region.start, region.frames, region.label));
    let loops = marks
        .loops
        .iter()
        .map(|found| format!("loop {} {}", found.start, found.end));
    regions.chain(loops).collect::<Vec<_>>().join("\n")
}

/// The marks a [`DETECTED`] set's value holds.
fn detected_marks(value: &str) -> Result<wav::Marks, String> {
    let mut marks = wav::Marks::default();
    for line in value.lines().filter(|line| !line.trim().is_empty()) {
        let bad = || format!("{DETECTED}: {line:?} is no region or loop");
        let mut words = line.split_whitespace();
        let first = words.next().ok_or_else(bad)?;
        let number = |word: Option<&str>| word.and_then(|w| w.parse::<u32>().ok()).ok_or_else(bad);
        if first == "loop" {
            let (start, end) = (number(words.next())?, number(words.next())?);
            marks.loops.push(wav::Loop::forward(start, end));
            continue;
        }
        let start = number(Some(first))?;
        let frames = number(words.next())?;
        let label = words.next().ok_or_else(bad)?.to_string();
        marks.regions.push(wav::Region {
            start,
            frames,
            label,
        });
    }
    Ok(marks)
}

/// `marks` with every region of the sample whose attack is `label` renamed to the sample
/// whose attack label is `to`.
fn renamed(marks: &wav::Marks, path: &str, label: &str, to: &str) -> Result<wav::Marks, String> {
    let from = take::mark(label, 0..0)
        .hit
        .ok_or_else(|| format!("{path}: names no sample"))?;
    let to = take::mark(to, 0..0)
        .hit
        .ok_or_else(|| format!("{path}: {to} names no sample"))?;
    let mut out = marks.clone();
    if from == to {
        return Ok(out);
    }
    if out
        .regions
        .iter()
        .any(|region| take::mark(&region.label, 0..0).hit == Some(to))
    {
        return Err(format!(
            "{path}: the take already marks {}",
            take::named(to)
        ));
    }
    for region in &mut out.regions {
        let mark = take::mark(&region.label, 0..0);
        if mark.hit != Some(from) {
            continue;
        }
        region.label = match mark.part {
            take::Part::Attack => adding::attack_label(to),
            take::Part::Resonance => encode::stroke_name(to.0, Bank::Resonance, to.1),
            take::Part::Release => adding::release_label(to),
            take::Part::Fade => adding::attack_label(to) + encode::FADE_SUFFIX,
            take::Part::Other => continue,
        };
    }
    Ok(out)
}

/// The word a [`DELETED`] path names `piece` by.
fn piece_slug(piece: hit::Piece) -> &'static str {
    match piece {
        hit::Piece::Note => "note",
        hit::Piece::Loop => "loop",
        hit::Piece::KeyOff => "key-off",
    }
}

/// `start..end`, as an add's value gives frames.
fn span_value(span: &Range<u32>) -> String {
    format!("{}..{}", span.start, span.end)
}

/// The frames `value` gives for the add `path`, of at least one frame.
fn frames_in(path: &str, value: &str) -> Result<Range<u32>, String> {
    let bad = || format!("{path}: {value} is not frames as start..end");
    let (start, end) = value.trim().split_once("..").ok_or_else(bad)?;
    let (start, end): (u32, u32) = (
        start.parse().map_err(|_| bad())?,
        end.parse().map_err(|_| bad())?,
    );
    match start < end {
        true => Ok(start..end),
        false => Err(bad()),
    }
}

/// What a set that adds or deletes a region or a loop says, for Undo to name it.
fn said_piece(path: &str, value: &str) -> Option<String> {
    if path == DETECTED {
        return Some("detect".to_string());
    }
    let name = |label: &str| take::mark(label, 0..0).hit.map(take::named);
    if let Some(label) = path.strip_prefix(RENAMED) {
        return Some(format!(
            "rename {} to {}",
            name(label).unwrap_or(label.to_string()),
            name(value).unwrap_or(value.to_string())
        ));
    }
    if let Some(label) = path.strip_prefix(ADDED) {
        let mark = take::mark(label, 0..0);
        return Some(match (mark.hit, mark.part) {
            (Some(hit), take::Part::Release) => format!("add {} key-off", take::named(hit)),
            (Some(hit), _) => format!("add {}", take::named(hit)),
            (None, _) => format!("add {label}"),
        });
    }
    if let Some(label) = path.strip_prefix(LOOPED) {
        return Some(format!(
            "add {} loop",
            name(label).unwrap_or(label.to_string())
        ));
    }
    let (label, piece) = path.strip_prefix(DELETED)?.rsplit_once('/')?;
    let name = name(label).unwrap_or(label.to_string());
    Some(match piece {
        "note" => format!("delete {name}"),
        piece => format!("delete {name} {piece}"),
    })
}

/// `marks` with the add or delete `path` with `value` made, or none where `path` is not
/// one.
fn pieced(marks: &wav::Marks, path: &str, value: &str) -> Option<Result<wav::Marks, String>> {
    if path == DETECTED {
        return Some(detected_marks(value));
    }
    if let Some(label) = path.strip_prefix(RENAMED) {
        return Some(renamed(marks, path, label, value));
    }
    let mut out = marks.clone();
    if let Some(label) = path.strip_prefix(ADDED) {
        return Some(frames_in(path, value).and_then(|span| {
            if out.regions.iter().any(|region| region.label == label) {
                return Err(format!("{path}: the take already marks {label}"));
            }
            out.regions.push(wav::Region {
                start: span.start,
                frames: span.end - span.start,
                label: label.to_string(),
            });
            Ok(out)
        }));
    }
    if let Some(label) = path.strip_prefix(LOOPED) {
        return Some(frames_in(path, value).and_then(|span| {
            let region = out
                .regions
                .iter()
                .find(|region| region.label == label)
                .ok_or_else(|| format!("{path}: the take marks no {label}"))?;
            let end = u64::from(region.start) + u64::from(region.frames);
            if span.start < region.start || u64::from(span.end) > end || span.len() < 2 {
                return Err(format!("{path}: a loop lies inside {label}"));
            }
            // ⚠️ A loop's end is its last frame, not the one after.
            out.loops.push(wav::Loop::forward(span.start, span.end - 1));
            Ok(out)
        }));
    }
    let rest = path.strip_prefix(DELETED)?;
    let mut deleted = || {
        let (label, piece) = rest
            .rsplit_once('/')
            .ok_or_else(|| format!("{path}: names no piece of a sample"))?;
        let hit = take::mark(label, 0..0)
            .hit
            .ok_or_else(|| format!("{path}: names no sample"))?;
        let cuts = hit::Cuts::of(&take::Marked::from_marks(marks), hit);
        let inside = |found: &wav::Loop| {
            cuts.attack
                .as_ref()
                .is_some_and(|attack| found.start >= attack.start && found.end < attack.end)
        };
        let of = |region: &wav::Region, part: Option<take::Part>| {
            let mark = take::mark(&region.label, 0..0);
            mark.hit == Some(hit) && part.is_none_or(|part| mark.part == part)
        };
        match piece {
            "note" => {
                out.regions.retain(|region| !of(region, None));
                out.loops.retain(|found| !inside(found));
            }
            "loop" => out.loops.retain(|found| !inside(found)),
            "key-off" => out
                .regions
                .retain(|region| !of(region, Some(take::Part::Release))),
            _ => {
                return Err(format!(
                    "{path}: {piece} is not a note, a loop or a key-off"
                ))
            }
        }
        Ok(out.clone())
    };
    Some(deleted())
}

/// The path that moves `cut` of the hit whose attack is labeled `label`.
pub fn cut_path(label: &str, cut: hit::Cut) -> String {
    format!("{CUT}{label}/{}", cut.slug())
}

/// The path of the kind of instrument a take is heard and built as, by the name
/// `nord piano build --kind` takes: `wurlitzer`.
pub const KIND: &str = "kind";

/// The path of whether the build cleans a take: `on` or `off`.
pub const CLEAN: &str = "clean";

/// The gains the control offers, in dB.
const GAINS: RangeInclusive<f64> = -24.0..=24.0;

/// Whether these bytes are in a RIFF/WAVE container.
///
/// Tests the container only: a compressed WAV still opens as one, and its document says
/// why it does not read.
pub fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE"
}

/// Whether these bytes mark a region labeled as a stroke: a take `nord piano prep` has
/// cut, against one as recorded.
pub fn is_take(bytes: &[u8]) -> bool {
    take::Marked::of(bytes).is_ok_and(|marked| marked.regions.iter().any(|mark| mark.hit.is_some()))
}

/// What `db` multiplies a sample by, where the control offers that gain.
fn factor(db: f64) -> Result<f64, String> {
    match GAINS.contains(&db) {
        true => Ok(10f64.powf(db / 20.0)),
        false => Err(format!(
            "a gain of {db} dB: the gain runs from {} to {} dB",
            GAINS.start(),
            GAINS.end()
        )),
    }
}

/// The values an integer sample of `bits` holds. 8-bit samples are stored unsigned, and
/// are counted here from their midpoint, as the wider ones are.
fn span(bits: u16) -> RangeInclusive<i64> {
    let half = 1i64 << (bits - 1);
    -half..=half - 1
}

/// `sample` times `factor`, rounded, and whether it was clipped to fit `bits`.
fn scaled(sample: i32, factor: f64, bits: u16) -> (i32, bool) {
    let exact = (f64::from(sample) * factor).round();
    let span = span(bits);
    let held = exact.clamp(*span.start() as f64, *span.end() as f64);
    (held as i32, held != exact)
}

/// The integer sample stored in `at`, `bits` wide.
fn int(at: &[u8], bits: u16) -> i32 {
    match bits {
        8 => i32::from(at[0]) - 128,
        16 => i32::from(i16::from_le_bytes([at[0], at[1]])),
        24 => i32::from_le_bytes([0, at[0], at[1], at[2]]) >> 8,
        _ => i32::from_le_bytes([at[0], at[1], at[2], at[3]]),
    }
}

/// Store `sample` in `at`, `bits` wide.
fn put_int(at: &mut [u8], sample: i32, bits: u16) {
    match bits {
        8 => at[0] = (sample + 128) as u8,
        16 => at.copy_from_slice(&(sample as i16).to_le_bytes()),
        24 => at.copy_from_slice(&sample.to_le_bytes()[..3]),
        _ => at.copy_from_slice(&sample.to_le_bytes()),
    }
}

/// Each integer sample of the data chunk, in file order.
fn ints<'a>(bytes: &'a [u8], data: Range<usize>, bits: u16) -> impl Iterator<Item = i32> + 'a {
    bytes[data]
        .chunks_exact(usize::from(bits / 8))
        .map(move |at| int(at, bits))
}

/// Scale every sample of `data` by `factor` in the encoding it is stored in, and count
/// the samples clipped. Float holds any level, so nothing clips there.
fn gain_in_place(data: &mut [u8], encoding: Encoding, factor: f64) -> usize {
    let mut clipped = 0;
    match encoding {
        Encoding::Int { bits } => {
            for at in data.chunks_exact_mut(usize::from(bits / 8)) {
                let (held, clips) = scaled(int(at, bits), factor, bits);
                put_int(at, held, bits);
                clipped += usize::from(clips);
            }
        }
        Encoding::Float { bits: 32 } => {
            for at in data.as_chunks_mut::<4>().0 {
                let x = f64::from(f32::from_le_bytes(*at)) * factor;
                *at = (x as f32).to_le_bytes();
            }
        }
        Encoding::Float { .. } => {
            for at in data.as_chunks_mut::<8>().0 {
                *at = (f64::from_le_bytes(*at) * factor).to_le_bytes();
            }
        }
    }
    clipped
}

/// `current` with `sets` applied, and the gain in dB where one was set.
///
/// A gain is applied to `original`, and `current`'s marks and settings carried over where
/// they differ from its own. A moved cut moves it in `current`'s marks, and a kind or a
/// clean is written to its settings; neither changes the audio.
///
/// ⚠️ A gain is a total, never a step: each one is applied to `original`, so a second
/// gain replaces the first instead of stacking on it, and nothing the first clipped or
/// rounded is carried into the second.
pub fn apply(
    original: &[u8],
    current: &[u8],
    sets: &[(String, String)],
) -> Result<(Vec<u8>, Option<f64>), String> {
    let error = |e: nord_format::error::Error| e.to_string();
    let layout = wav::layout(original).map_err(error)?;
    let mut total = None;
    let mut marks: Option<wav::Marks> = None;
    let mut settings = wav::read_settings(current).map_err(error)?;
    for (path, value) in sets {
        if path == KIND {
            if encode::Kind::from_word(value).is_none() {
                return Err(format!("{KIND}: {value} is no kind of instrument"));
            }
            settings.kind = Some(value.clone());
            continue;
        }
        if path == CLEAN {
            settings.clean = Some(match value.as_str() {
                "on" => true,
                "off" => false,
                _ => return Err(format!("{CLEAN}: {value} is not on or off")),
            });
            continue;
        }
        if path == GAIN {
            let db: f64 = value
                .trim()
                .parse()
                .map_err(|_| format!("{GAIN}: {value} is not a number of dB"))?;
            factor(db)?;
            total = Some(db);
            continue;
        }
        let held = match marks.take() {
            Some(held) => Some(held),
            None if said_piece(path, value).is_some() => {
                Some(wav::read_marks(current).map_err(error)?)
            }
            None => None,
        };
        if let Some(held) = &held {
            if let Some(made) = pieced(held, path, value) {
                marks = Some(made?);
                continue;
            }
        }
        marks = held;
        let Some((label, slug)) = path
            .strip_prefix(CUT)
            .and_then(|rest| rest.rsplit_once('/'))
        else {
            return Err(format!(
                "{path}: a WAV's edits are its {GAIN}, its cuts, its {KIND} and {CLEAN}"
            ));
        };
        let (Some(hit), Some(cut)) = (take::mark(label, 0..0).hit, hit::Cut::from_slug(slug))
        else {
            return Err(format!("{path}: names no cut of a sample"));
        };
        let frame: u32 = value
            .trim()
            .parse()
            .map_err(|_| format!("{path}: {value} is not a frame"))?;
        let held = match marks.take() {
            Some(held) => held,
            None => wav::read_marks(current).map_err(error)?,
        };
        let frames = wav::layout(current).map_err(error)?.frames() as u32;
        marks = Some(hit::moved(&held, hit, cut, frame, frames)?);
    }
    let mut out = match total {
        Some(db) => {
            let mut out = original.to_vec();
            gain_in_place(&mut out[layout.data], layout.format.encoding, factor(db)?);
            let now = wav::read_marks(current).ok();
            match (&marks, now) {
                (None, Some(now)) if wav::read_marks(original).ok() != Some(now.clone()) => {
                    wav::with_marks(&out, &now).map_err(error)?
                }
                _ => out,
            }
        }
        None => current.to_vec(),
    };
    if let Some(marks) = &marks {
        out = wav::with_marks(&out, marks).map_err(error)?;
    }
    if wav::read_settings(&out).ok().as_ref() != Some(&settings) {
        out = wav::with_settings(&out, &settings).map_err(error)?;
    }
    Ok((out, total))
}

/// The path of the set that undoes the last edit.
pub const UNDO: &str = "undo";

/// The path of the set that makes again the edit undone last.
pub const REDO: &str = "redo";

/// The label of the button that undoes the last edit.
pub const UNDO_LABEL: &str = "Undo";

/// The take's loud action: Detect, which marks its samples in place of its regions.
pub(super) fn detect_loud(state: &State) -> super::header::Loud {
    use super::header::{Click, Loud, Tone};
    let ready = Loud {
        label: "Detect".to_string(),
        short: "Detect".to_string(),
        glyph: crate::icon::Glyph::Search,
        tone: Tone::Ready,
        hint: "finds the take's notes, layers, cuts and loops, and marks them in place of \
               its regions; Undo puts them back"
            .to_string(),
        click: Click::Detect,
    };
    if state.detect.running() {
        return Loud {
            label: "Detecting…".to_string(),
            short: "Detecting…".to_string(),
            tone: Tone::Idle,
            hint: "finding the take's notes".to_string(),
            click: Click::Nothing,
            ..ready
        };
    }
    if state.audio().is_none() {
        return Loud {
            tone: Tone::Idle,
            hint: "the WAV's audio does not read".to_string(),
            click: Click::Nothing,
            ..ready
        };
    }
    ready
}

/// What an edit undoes to: the gain, the marks and the settings the bytes held before
/// it, and what it was, for the Undo button to name.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub gain: f64,
    pub marks: Option<wav::Marks>,
    pub settings: Option<wav::Settings>,
    pub what: String,
    /// When on the app's timeline the edit was made, undone or made again.
    pub at: u64,
}

/// What an edit, an undo or a redo leaves for Undo and Redo.
#[derive(Clone, Debug, PartialEq)]
pub enum Stepped {
    /// An edit, and what undoing it goes back to. It ends the redo.
    Made(Step),
    /// The last edit undone, and what redoing it goes back to.
    Undone(Step),
    /// The edit undone last made again, and what undoing it goes back to.
    Redone(Step),
}

impl Step {
    /// What `sets` undo to on `current`, which holds `gain` against the original.
    pub fn before(current: &[u8], gain: f64, sets: &[(String, String)]) -> Step {
        let said: Vec<String> = sets
            .iter()
            .map(|(path, value)| match path.strip_prefix(CUT) {
                _ if said_piece(path, value).is_some() => {
                    said_piece(path, value).unwrap_or_default()
                }
                Some(rest) => {
                    let (label, slug) = rest.rsplit_once('/').unwrap_or((rest, ""));
                    let cut = hit::Cut::from_slug(slug).map_or(slug, |cut| cut.name());
                    format!("{} {cut}", take::mark(label, 0..0).text)
                }
                None if path == KIND => format!(
                    "category {}",
                    encode::Kind::from_word(value).map_or(value.clone(), |kind| {
                        super::piano::kind_name(kind.code())
                    })
                ),
                None if path == CLEAN => format!("clean audio {value}"),
                None => match value.trim().parse::<f64>() {
                    Ok(db) => format!("gain {db:+.1} dB"),
                    Err(_) => format!("gain {value}"),
                },
            })
            .collect();
        Step {
            gain,
            marks: wav::read_marks(current).ok(),
            settings: wav::read_settings(current).ok(),
            what: said.join(", "),
            at: crate::timeline::tick(),
        }
    }

    /// `current`, which holds `gain`, as the step an undo or redo of `step` leaves to
    /// go back to: the same edit, said the same way.
    pub fn back(current: &[u8], gain: f64, step: &Step) -> Step {
        Step {
            what: step.what.clone(),
            ..Step::before(current, gain, &[])
        }
    }
}

/// `original` at a total gain of `gain`, marking `marks` and stating `settings` where
/// they differ from its own: the bytes an edit was made on, made again.
pub fn restored(original: &[u8], step: &Step) -> Result<Vec<u8>, String> {
    let sets = [(GAIN.to_string(), format!("{:.1}", step.gain))];
    let (mut out, _) = apply(original, original, &sets)?;
    if let Some(marks) = &step.marks {
        if wav::read_marks(original).ok().as_ref() != Some(marks) {
            out = wav::with_marks(&out, marks).map_err(|e| e.to_string())?;
        }
    }
    if let Some(settings) = &step.settings {
        if wav::read_settings(&out).ok().as_ref() != Some(settings) {
            out = wav::with_settings(&out, settings).map_err(|e| e.to_string())?;
        }
    }
    Ok(out)
}

/// The identity cells a WAV puts on the header: the rate, channels, encoding and length
/// its container declares, and what the take marks, read without copying its samples.
pub fn stated(entity: &LocalEntity) -> Vec<Cell> {
    if Kind::of(entity) != Kind::Wav {
        return Vec::new();
    }
    let Ok(layout) = wav::layout(&entity.bytes) else {
        return Vec::new();
    };
    let take = match take::Marked::of(&entity.bytes) {
        Ok(marked) => marked.stated(),
        Err(_) => "its regions do not read".to_string(),
    };
    vec![
        Cell {
            label: "Audio",
            body: Body::Read(facts(&layout)),
            hint: "the rate, channels, bit depth and length the file declares",
        },
        Cell {
            label: "Take",
            body: Body::Read(take),
            hint:
                "the samples, regions and loops the file marks; a raw take, as recorded, has none",
        },
    ]
}

/// `44100 Hz · stereo · 24-bit · 1.500 s`.
fn facts(layout: &Layout) -> String {
    let format = layout.format;
    let channels = match format.channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n} channels"),
    };
    let encoding = match format.encoding {
        Encoding::Int { bits } => format!("{bits}-bit"),
        Encoding::Float { bits } => format!("{bits}-bit float"),
    };
    let seconds = layout.frames() as f64 / f64::from(format.rate.max(1));
    format!(
        "{} Hz · {channels} · {encoding} · {seconds:.3} s",
        format.rate
    )
}

/// What a WAV's document keeps between frames: the gain asked for, the bytes every gain
/// is applied to, and a read of the bytes it is open on.
///
/// Nothing here outlives the document: what the working copy holds against the saved
/// file is worked out again from the two whenever the document opens, so a WAV closed
/// unsaved, or kept unsaved across a restart, opens on its gain.
pub struct State {
    /// The total gain asked for against `original`, in dB to one decimal.
    pub gain: f64,
    /// The total gain the bytes hold against `original`.
    applied: f64,
    /// The file as last saved, which Revert goes back to; or, where those bytes cannot
    /// be had or the working copy is not a gain of them, the working copy itself.
    original: Bytes,
    /// The stamps of the bytes made here from `original`, and the gain each holds, so a
    /// revert to one of them shows its gain again.
    made: Vec<(u64, f64)>,
    /// What each edit since the document opened undoes to, the last one last.
    history: Vec<Step>,
    /// What each edit undone since the last one made redoes to, the one undone last last.
    redo: Vec<Step>,
    /// When on the app's timeline the last edit, not an undo or redo, was made.
    changed: Option<u64>,
    notes: Notes,
    read: Read,
    /// What the take's lane shows, kept across new bytes.
    pub view: take::View,
    /// What the picked hit's lane shows, kept across new bytes.
    pub close: hit::Close,
    /// What the take is heard as, kept across new bytes.
    pub audition: audition::Audition,
    /// What Detect asks for and found, kept across new bytes.
    pub detect: Detect,
}

/// Detect on a take: the too-quiet limit it uses, the search running, and what the last
/// one found.
pub struct Detect {
    /// A note standing this little over the floor is skipped, in dB.
    pub limit: f64,
    running: Option<crate::work::Job<crate::work::Detected>>,
    /// What the last search found, or why it found nothing.
    pub found: Option<Result<crate::work::Detected, String>>,
}

impl Default for Detect {
    fn default() -> Detect {
        Detect {
            limit: nord_format::take::plan::MIN_SNR_DB,
            running: None,
            found: None,
        }
    }
}

impl Detect {
    /// Whether a search is running.
    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    /// Start finding the samples of `audio`, unless a search is running.
    pub fn start(&mut self, ctx: &egui::Context, audio: &Audio) {
        if self.running.is_none() {
            let work = crate::work::FindHits::of(audio, self.limit);
            self.running = Some(crate::work::hand(ctx, work));
            self.found = None;
        }
    }

    /// Take what the search found, once it has, as the set that marks it.
    pub fn follow(&mut self, sets: &mut Sets) {
        let Some(job) = &self.running else {
            return;
        };
        let found = match job.poll() {
            crate::work::Answer::Running => return,
            crate::work::Answer::Answered(found) => found,
            crate::work::Answer::Died => {
                self.running = None;
                self.found = Some(Err("Detect stopped without a result".to_string()));
                return;
            }
        };
        self.running = None;
        if found.kept > 0 {
            sets.push((DETECTED.to_string(), found.value.clone()));
        }
        self.found = Some(Ok(found));
    }

    /// What the last search found, in words.
    pub fn said(&self) -> Option<String> {
        if self.running() {
            return Some("finding the take's notes…".to_string());
        }
        Some(match self.found.as_ref()? {
            Err(why) => why.clone(),
            Ok(found) if found.kept == 0 => format!(
                "Detect marked no sample: skipped {} at {:.0} dB or less over noise; the \
                 take's regions are as they were",
                found.too_quiet, self.limit
            ),
            Ok(found) => format!(
                "Detect marked {} sample(s) of {} note(s){}",
                found.kept,
                found.notes,
                match found.too_quiet {
                    0 => String::new(),
                    n => format!(", skipped {n} at {:.0} dB or less over noise", self.limit),
                }
            ),
        })
    }
}

/// The notes of the original, each by its quietest and loudest integer sample, so the
/// notes a gain clips are counted before it is applied.
///
/// A note runs from its onset, as `nord piano prep` finds it, to the next one; what comes
/// before the first belongs to the first. A file with no onset is one note. A note clips
/// where its loudest sample of either sign does.
#[derive(Default)]
struct Notes {
    bits: u16,
    peaks: Vec<(i32, i32)>,
}

impl Notes {
    fn of(bytes: &[u8], audio: &Audio) -> Notes {
        let Ok(layout) = wav::layout(bytes) else {
            return Notes::default();
        };
        let Encoding::Int { bits } = layout.format.encoding else {
            return Notes::default();
        };
        let x = detect::highpassed(&nord_format::take::plan::mono(audio), audio.rate);
        let env = Envelope::of(&x, audio.rate);
        // ⚠️ A file shorter than one envelope frame has no floor to find onsets over.
        let onsets = match env.db.is_empty() {
            true => Vec::new(),
            false => detect::onsets(&env),
        };
        let mut starts: Vec<usize> = onsets.iter().map(|onset| onset * env.width).collect();
        match starts.first_mut() {
            Some(first) => *first = 0,
            None => starts.push(0),
        }
        let channels = usize::from(layout.format.channels);
        let mut peaks = vec![(0, 0); starts.len()];
        let mut note = 0;
        for (at, sample) in ints(bytes, layout.data, bits).enumerate() {
            while starts
                .get(note + 1)
                .is_some_and(|next| at / channels >= *next)
            {
                note += 1;
            }
            let (low, high) = &mut peaks[note];
            *low = sample.min(*low);
            *high = sample.max(*high);
        }
        Notes { bits, peaks }
    }

    /// How many notes a gain of `factor` clips.
    fn clipped(&self, factor: f64) -> usize {
        let clips = |sample: i32| scaled(sample, factor, self.bits).1;
        self.peaks
            .iter()
            .filter(|(low, high)| clips(*low) || clips(*high))
            .count()
    }
}

/// A megabyte as the computer's own file sizes count one.
const MB: usize = 1 << 20;

/// The largest WAV drawbar-web opens, in bytes. Opening a take costs the page about nine
/// times its file in memory, and finding its noise as much again in its worker
/// (measured 2026-11-13 on a 132 MB take of 522 s, 24-bit stereo at 44.1 kHz: 1.2 GB
/// each), and a browser tab gets at most 4 GB, often far less. A WAV of a few notes is
/// far smaller: past this, a take is for the desktop.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const WEB_MOST_BYTES: usize = 120 * MB;

/// Why a WAV of `len` bytes is not opened, where it is more than `most`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn too_large(len: usize, most: usize) -> Option<String> {
    (len > most).then(|| {
        // Whole megabytes cut down, as the computer's own file sizes show them, unless
        // that would show the file no larger than the limit.
        let size = match len / MB > most / MB {
            true => format!("{}", len / MB),
            false => format!("{:.1}", (len as f64 / MB as f64 * 10.0).floor() / 10.0),
        };
        format!(
            "This WAV is {size} MB, more than the browser can work on ({} MB). Use the Desktop version of Drawbar instead.",
            most / MB
        )
    })
}

/// One set of a WAV's bytes, read.
///
/// ⚠️ Reading copies every sample, so it happens when the bytes change and never per
/// frame.
struct Read {
    /// The [`LocalEntity::stamp`] of the bytes read.
    stamp: u64,
    /// The audio as float, or why the bytes do not read.
    source: Result<Audio, String>,
    /// Whether `source` was not read because the browser cannot spare the memory, so
    /// its error is said as it stands.
    too_large: bool,
    /// The regions and loops the file marks, as it holds them and as the lanes draw them,
    /// or why they do not read.
    marks: Result<wav::Marks, String>,
    marked: Result<take::Marked, String>,
    /// What the take states about its build, nothing where it states nothing or its
    /// settings do not read.
    settings: wav::Settings,
    summary: take::Summary,
    drawn: take::Drawn,
    /// The envelopes last drawn on the hit's lane.
    close: take::Drawn,
}

impl Read {
    fn of(entity: &LocalEntity) -> Read {
        #[cfg(target_arch = "wasm32")]
        let refused = too_large(entity.bytes.len(), WEB_MOST_BYTES);
        #[cfg(not(target_arch = "wasm32"))]
        let refused = None;
        let too_large = refused.is_some();
        let source = match refused {
            Some(why) => Err(why),
            None => wav::read(&entity.bytes).map_err(|e| e.to_string()),
        };
        let marks = wav::read_marks(&entity.bytes).map_err(|e| e.to_string());
        Read {
            stamp: entity.stamp,
            too_large,
            summary: source.as_ref().map(take::Summary::of).unwrap_or_default(),
            source,
            marked: marks
                .as_ref()
                .map(take::Marked::from_marks)
                .map_err(Clone::clone),
            marks,
            settings: wav::read_settings(&entity.bytes).unwrap_or_default(),
            drawn: take::Drawn::default(),
            close: take::Drawn::default(),
        }
    }
}

/// The file `entity` was last saved as, where it is not the working copy: held, or read
/// back from its file. `None` where neither has it.
///
/// ⚠️ Reading it back reads the whole file, on this thread.
fn saved(entity: &LocalEntity) -> Option<Bytes> {
    let held = entity.saved.bytes();
    if !held.is_empty() {
        return Some(held.clone());
    }
    let file = entity.saved.file()?;
    file.read(0..file.len).ok().map(Bytes::from)
}

/// The gain in dB, to the control's step, that makes `edited` of `original`, where one
/// does.
///
/// Measured on the loudest sample the gain left unclipped, then proved: the original at
/// that gain must be the edited bytes exactly, so an edit that is not a gain, or a gain
/// off the control's steps, is never taken for one.
fn held_gain(original: (&[u8], &Audio), edited: (&[u8], &Audio)) -> Option<f64> {
    let ((was, before), (now, after)) = (original, edited);
    if before.samples.len() != after.samples.len() {
        return None;
    }
    let (from, to) = before
        .samples
        .iter()
        .zip(&after.samples)
        .filter(|(_, to)| to.abs() < 0.99)
        .max_by(|(a, _), (b, _)| a.abs().total_cmp(&b.abs()))?;
    let ratio = f64::from(*to) / f64::from(*from);
    if ratio.is_nan() || ratio <= 0.0 {
        return None;
    }
    let db = (20.0 * ratio.log10() * 10.0).round() / 10.0;
    // With the cuts the edited bytes mark, so moved cuts and a gain both count.
    let (made, _) = apply(was, now, &[(GAIN.to_string(), format!("{db:.1}"))]).ok()?;
    (made == now).then_some(db)
}

impl State {
    /// ⚠️ Reads the working copy and, where it is unsaved, the saved file, and finds the
    /// notes of the whole file, so it runs when the bytes come from elsewhere and never
    /// per frame.
    pub fn new(entity: &LocalEntity) -> State {
        let read = Read::of(entity);
        let mut original = (entity.bytes.clone(), 0.0);
        let mut heard = None;
        if let (true, Some(saved), Ok(now)) = (entity.is_unsaved(), saved(entity), &read.source) {
            if let Ok(before) = wav::read(&saved) {
                if let Some(db) = held_gain((&saved, &before), (&entity.bytes, now)) {
                    heard = Some(before);
                    original = (saved, db);
                }
            }
        }
        let (original, applied) = original;
        let notes = match (&heard, &read.source) {
            (Some(audio), _) | (None, Ok(audio)) => Notes::of(&original, audio),
            (None, Err(_)) => Notes::default(),
        };
        State {
            gain: applied,
            applied,
            original,
            made: vec![(entity.stamp, applied)],
            history: Vec::new(),
            redo: Vec::new(),
            changed: None,
            notes,
            read,
            view: take::View::default(),
            close: hit::Close::default(),
            audition: audition::Audition::default(),
            detect: Detect::default(),
        }
    }

    /// Read the bytes again where they are no longer the ones read. Bytes this document
    /// made keep the original and show their gain. Any others, and saved bytes, which
    /// the next gain starts from, open the document again.
    pub fn follow(&mut self, entity: &LocalEntity) {
        let saved = self.applied != 0.0 && !entity.is_unsaved();
        if self.read.stamp == entity.stamp && !saved {
            return;
        }
        match self.made.iter().find(|(stamp, _)| *stamp == entity.stamp) {
            Some(&(_, total)) if !saved => {
                self.applied = total;
                self.gain = total;
                self.read = Read::of(entity);
            }
            _ => {
                let (view, close, audition, detect) = (
                    std::mem::take(&mut self.view),
                    std::mem::take(&mut self.close),
                    std::mem::take(&mut self.audition),
                    std::mem::take(&mut self.detect),
                );
                *self = State::new(entity);
                (self.view, self.close, self.audition, self.detect) =
                    (view, close, audition, detect);
            }
        }
    }

    /// The bytes every gain is applied to.
    pub fn original(&self) -> &[u8] {
        &self.original
    }

    /// The total gain the bytes hold against the original.
    pub fn applied(&self) -> f64 {
        self.applied
    }

    /// What the last edit undoes to, where there is one.
    pub fn last_step(&self) -> Option<&Step> {
        self.history.last()
    }

    /// What Redo makes again, where an edit was undone since the last one made.
    pub fn last_redo(&self) -> Option<&Step> {
        self.redo.last()
    }

    /// When the last edit not undone, and the last undo, were, on the app's timeline.
    pub fn timeline(&self) -> crate::timeline::Held {
        crate::timeline::Held {
            made: self.history.last().map(|step| step.at),
            undone: self.redo.last().map(|step| step.at),
            changed: self.changed,
        }
    }

    /// Note an edit made, undone or made again.
    pub fn stepped(&mut self, stepped: Stepped) {
        match stepped {
            Stepped::Made(step) => {
                self.changed = Some(step.at);
                self.history.push(step);
                self.redo.clear();
            }
            Stepped::Undone(back) => {
                self.history.pop();
                self.redo.push(back);
            }
            Stepped::Redone(back) => {
                self.redo.pop();
                self.history.push(back);
            }
        }
    }

    /// Note that the bytes stamped `stamp` hold `total` against the original.
    pub fn made(&mut self, stamp: u64, total: f64) {
        self.made.retain(|(made, _)| *made != stamp);
        self.made.push((stamp, total));
        self.applied = total;
    }

    /// What Play plays: the picked sample as recorded, from its first cut to its last;
    /// where a place is clicked instead, what the lane under the take shows about it; or
    /// the whole file where neither is.
    pub fn heard(&self) -> Option<&[f32]> {
        let audio = self.audio()?;
        let channels = usize::from(audio.channels.max(1));
        let len = audio.samples.len();
        let picked = self
            .view
            .picked
            .zip(self.read.marked.as_ref().ok())
            .and_then(|(hit, marked)| marked.extent(hit));
        let placed = || {
            let place = self.view.place.filter(|_| self.view.picked.is_none())?;
            let rate = f64::from(audio.rate.max(1));
            let about = self.close.about(place, (len / channels) as f64, rate);
            Some(about.start.floor() as u32..about.end.ceil() as u32)
        };
        let span = picked.or_else(placed);
        Some(match span {
            Some(span) => {
                let at = |frame: u32| (frame as usize * channels).min(len);
                &audio.samples[at(span.start)..at(span.end)]
            }
            None => &audio.samples,
        })
    }

    /// The picked sample's cuts, where one is picked in marks that read.
    pub fn cuts(&self) -> Option<hit::Cuts> {
        let marked = self.read.marked.as_ref().ok()?;
        Some(hit::Cuts::of(marked, self.view.picked?))
    }

    /// Start finding the take's noise where it is cleaned, or take it once found.
    pub fn follow_noise(&mut self, ctx: &egui::Context) {
        let clean = self.clean();
        if let Ok(audio) = &self.read.source {
            self.audition.follow(audio, clean, ctx);
        }
    }

    /// Make the picked sample ready to play, again only where the bytes, the pick, the
    /// kind or the cleaning changed, so a key is heard as soon as it goes down. Nothing
    /// while the take's noise is still being found.
    pub fn prepare(&mut self) {
        let (Some(hit), Ok(audio)) = (self.view.picked, &self.read.source) else {
            return;
        };
        let clean = self.clean();
        let made = (self.read.stamp, hit, self.kind(), clean);
        if self
            .audition
            .ready
            .as_ref()
            .is_some_and(|(at, _)| *at == made)
        {
            return;
        }
        let sounding = match self.audition.cleaning(audio, clean) {
            audition::Cleaning::Off => self.sounding(None),
            audition::Cleaning::Ready(noise) => self.sounding(noise),
            audition::Cleaning::Finding => return,
        };
        self.audition.ready = Some((made, sounding));
    }

    /// The kind of instrument the take states it is, grand where it states none.
    pub fn kind(&self) -> encode::Kind {
        self.read
            .settings
            .kind
            .as_deref()
            .and_then(encode::Kind::from_word)
            .unwrap_or_default()
    }

    /// Whether the take is cleaned of its hum and hiss, as it is unless it says not.
    pub fn clean(&self) -> bool {
        self.read.settings.clean.unwrap_or(true)
    }

    /// The picked sample as the instrument will play it, heard as the take's kind, and
    /// cleaned of `noise` where it is given.
    ///
    /// ⚠️ Shapes the sample's audio: run when it is asked to play, not per frame.
    pub fn sounding(
        &self,
        noise: Option<&nord_format::take::clean::Noise>,
    ) -> Option<audition::Sounding> {
        let (cuts, audio, root) = (self.cuts()?, self.audio()?, self.view.picked?.0);
        let rules = audition::Sounding::rules(self.kind());
        let marked = self.read.marked.as_ref().ok()?;
        let channels = usize::from(audio.channels.max(1));
        let loudest = marked
            .regions
            .iter()
            .filter(|mark| {
                mark.hit.is_some_and(|hit| hit.0 == root)
                    && matches!(mark.part, take::Part::Attack | take::Part::Resonance)
            })
            .flat_map(|mark| {
                let at = |frame: u32| (frame as usize * channels).min(audio.samples.len());
                &audio.samples[at(mark.frames.start)..at(mark.frames.end)]
            })
            .fold(0.0f32, |most, v| most.max(v.abs()));
        audition::Sounding::of(audio, &cuts, root, &rules, noise, loudest)
    }

    /// Whether the file marks samples, so the take's cards show.
    pub fn marks_samples(&self) -> bool {
        self.read
            .marked
            .as_ref()
            .is_ok_and(|marked| marked.regions.iter().any(|mark| mark.hit.is_some()))
    }

    /// Start Detect on the take, where its audio reads.
    pub fn start_detect(&mut self, ctx: &egui::Context) {
        if let Ok(audio) = &self.read.source {
            self.detect.start(ctx, audio);
        }
    }

    /// The audio as float, whatever the file stores it as.
    pub fn audio(&self) -> Option<&Audio> {
        self.read.source.as_ref().ok()
    }

    /// What a total gain of `db` would clip, in notes, or `None` where nothing clips.
    pub fn clipping(&self, db: f64) -> Result<Option<String>, String> {
        let clipped = self.notes.clipped(factor(db)?);
        let notes = self.notes.peaks.len();
        Ok(match (clipped, notes) {
            (0, _) => None,
            (_, 1) => Some("it clips".to_string()),
            (1, _) => Some(format!("1 of {notes} notes clips")),
            _ => Some(format!("{clipped} of {notes} notes clip")),
        })
    }
}

/// Draw the document: the gain over the take's lane, the picked hit's lane and Play under
/// it where the bytes read,
/// and why not where they do not. `true` once the operator has asked to play the file, or
/// to stop it where it is playing.
pub fn ui(
    ui: &mut egui::Ui,
    state: &mut State,
    playing: bool,
    played: impl Fn(usize) -> Option<f64>,
    sets: &mut Sets,
) -> (bool, Option<audition::Ask>) {
    let mut audition = false;
    let mut asked = None;
    state.detect.follow(sets);
    if let Some(said) = state.detect.said() {
        let ink = match state.detect.found {
            Some(Err(_)) => crate::app::bad(ui.visuals()),
            _ => ui.visuals().weak_text_color(),
        };
        ui.label(egui::RichText::new(said).small().color(ink));
    }
    if state.marks_samples() {
        // From the take's opening, so its samples are ready to hear by the time one is
        // picked.
        state.follow_noise(ui.ctx());
    }
    // ⚠️ The gain sits over the take, not by Play: it is applied to the whole file, never
    // to the picked hit alone.
    if state.audio().is_some() {
        ui.horizontal(|ui| {
            gain(ui, state, sets);
            if state.marks_samples() {
                ui.add_space(12.0);
                if let Some(kind) = audition::category(ui, state.kind()) {
                    sets.push((KIND.to_string(), kind.word().to_string()));
                }
            }
        });
        ui.add_space(6.0);
    }
    let last = state.last_step().map(|step| step.what.clone());
    let read = &mut state.read;
    match &read.source {
        Ok(audio) => {
            let picked = state.view.picked;
            let place = state.view.place;
            // Both lanes draw a dragged cut where it is now; the file moves it when the
            // drag ends.
            let frames = (audio.samples.len() / usize::from(audio.channels.max(1))) as u32;
            let dragged = match (&read.marks, picked, state.close.dragging(picked)) {
                (Ok(marks), Some(hit), Some((cut, frame))) => {
                    hit::moved(marks, hit, cut, frame, frames)
                        .ok()
                        .map(|moved| Ok(take::Marked::from_marks(&moved)))
                }
                _ => None,
            };
            let marked = dragged.as_ref().unwrap_or(&read.marked);
            take::ui(
                ui,
                audio,
                &read.summary,
                &mut read.drawn,
                marked,
                &mut state.view,
            );
            ui.add_space(10.0);
            let (dropped, wanted) = hit::ui(
                ui,
                audio,
                &read.summary,
                &mut read.close,
                marked,
                picked,
                place,
                &mut state.close,
                last.as_deref(),
            );
            if let (Some(hit::Wanted::Rename(to)), Some(from), Ok(held)) =
                (wanted, picked, &read.marked)
            {
                let from = hit::Cuts::of(held, from);
                sets.push((format!("{RENAMED}{}", from.label), adding::attack_label(to)));
                state.view.picked = Some(to);
                state.close.renamed(to);
            } else if wanted == Some(hit::Wanted::Undo) {
                sets.push((UNDO.to_string(), String::new()));
            } else if let (Some(wanted), Ok(held)) = (wanted, &read.marked) {
                pieces(wanted, audio, held, &mut state.view, sets);
            }
            if let (Some((cut, frame)), Some(hit), Ok(held)) = (dropped, picked, &read.marked) {
                let cuts = hit::Cuts::of(held, hit);
                let was = cuts.points().into_iter().find(|(each, _)| *each == cut);
                if was.is_some_and(|(_, at)| at != frame) {
                    sets.push((cut_path(&cuts.label, cut), frame.to_string()));
                }
            }
        }
        Err(why) => {
            let said = match read.too_large {
                true => why.clone(),
                false => format!("This WAV does not read: {why}"),
            };
            ui.label(egui::RichText::new(said).color(app::bad(ui.visuals())));
        }
    }
    if state.audio().is_some() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let shown = state.view.picked.is_some() || state.view.place.is_some();
            let (label, glyph) = match (playing, shown) {
                (true, _) => ("Stop", Glyph::X),
                (false, true) => (PLAY_HIT, Glyph::AudioLines),
                (false, false) => ("Play", Glyph::AudioLines),
            };
            audition = sample::action(ui, label, glyph, app::accent(ui.visuals()));
            if state.marks_samples() {
                asked = audition::controls(ui, state, played);
                steps(ui, state);
            }
        });
    }
    if let Some(audition::Ask::Clean(on)) = asked {
        sets.push((CLEAN.to_string(), if on { "on" } else { "off" }.to_string()));
    }
    (audition, asked)
}

/// The sets that do what the close lane's menus asked of the picked sample, or of the
/// place clicked: a sample added there is picked, and a sample deleted leaves its place.
fn pieces(
    wanted: hit::Wanted,
    audio: &Audio,
    marked: &take::Marked,
    view: &mut take::View,
    sets: &mut Sets,
) {
    use hit::{Piece, Wanted};
    if let (Wanted::Add(Piece::Note), Some(place)) = (wanted, view.place) {
        let added = adding::sample(audio, marked, place);
        sets.push((format!("{ADDED}{}", added.label), span_value(&added.attack)));
        sets.push((
            format!("{ADDED}{}", adding::release_label(added.hit)),
            span_value(&added.release),
        ));
        (view.picked, view.place) = (Some(added.hit), None);
        return;
    }
    let Some(hit) = view.picked else {
        return;
    };
    let cuts = hit::Cuts::of(marked, hit);
    let Some(attack) = cuts.attack.clone() else {
        return;
    };
    match wanted {
        Wanted::Add(Piece::KeyOff) => sets.push((
            format!("{ADDED}{}", adding::release_label(hit)),
            span_value(&adding::key_off(audio, attack.start, attack.end)),
        )),
        Wanted::Add(Piece::Loop) => sets.push((
            format!("{LOOPED}{}", cuts.label),
            span_value(&adding::default_loop(&attack, audio.rate)),
        )),
        Wanted::Add(Piece::Note) | Wanted::Undo | Wanted::Rename(_) => {}
        Wanted::Delete(piece) => {
            sets.push((
                format!("{DELETED}{}/{}", cuts.label, piece_slug(piece)),
                String::new(),
            ));
            if piece == Piece::Note {
                (view.picked, view.place) = (None, Some(attack.start));
            }
        }
    }
}

/// What the arrow that picks the sample before the picked one says, on hover.
pub const BEFORE: &str = "Previous sample";

/// What the arrow that picks the sample after the picked one says, on hover.
pub const AFTER: &str = "Next sample";

/// The small arrows at the far right of the row under the picked sample, which pick the
/// sample before and after it in the take.
fn steps(ui: &mut egui::Ui, state: &mut State) {
    let Some((total, rate)) = state.audio().map(|audio| {
        let channels = usize::from(audio.channels.max(1));
        (
            (audio.samples.len() / channels) as f64,
            f64::from(audio.rate.max(1)),
        )
    }) else {
        return;
    };
    let Ok(marked) = &state.read.marked else {
        return;
    };
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        // ⚠️ Right to left: the later arrow is added first.
        for (word, hint, forward) in [("→", AFTER, true), ("←", BEFORE, false)] {
            let next = state.view.beside(marked, forward);
            if ui
                .add_enabled(next.is_some(), egui::Button::new(word).small())
                .on_hover_text(hint)
                .clicked()
            {
                if let Some(hit) = next {
                    state.view.pick(hit, marked, total, rate);
                }
            }
        }
    });
}

/// The label of Play while a sample is picked: it plays that sample as the take holds it,
/// from its first cut to its last, unshaped.
pub const PLAY_HIT: &str = "Play original";

/// The label of the button that applies the gain.
pub const APPLY_GAIN: &str = "Apply gain";

/// The gain control, the notes it would clip, and the button that applies it.
///
/// ⚠️ Applied by a click and not as the control moves: every sample is written again.
fn gain(ui: &mut egui::Ui, state: &mut State, sets: &mut Sets) {
    ui.label("Gain");
    ui.add(
        egui::DragValue::new(&mut state.gain)
            .range(GAINS)
            .speed(0.1)
            .fixed_decimals(1)
            .suffix(" dB"),
    );
    state.gain = (state.gain * 10.0).round() / 10.0;
    let apply = ui.add_enabled(state.gain != state.applied, egui::Button::new(APPLY_GAIN));
    if apply.clicked() {
        sets.push((GAIN.to_string(), format!("{:.1}", state.gain)));
        return;
    }
    let (said, ink) = match state.clipping(state.gain) {
        Ok(None) => ("nothing clips".to_string(), app::caption(ui.visuals())),
        Ok(Some(clipped)) => (clipped, app::warn(ui.visuals())),
        Err(why) => (why, app::bad(ui.visuals())),
    };
    ui.label(egui::RichText::new(said).small().color(ink));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_too_large_for_the_browser_says_its_size_and_where_to_open_it() {
        assert_eq!(too_large(WEB_MOST_BYTES, WEB_MOST_BYTES), None);
        assert_eq!(too_large(104 * MB + MB / 2, WEB_MOST_BYTES), None);
        assert_eq!(
            too_large(138_193_000, WEB_MOST_BYTES).as_deref(),
            Some(
                "This WAV is 131 MB, more than the browser can work on (120 MB). Use the Desktop version of Drawbar instead."
            )
        );
        let just = too_large(120 * MB + MB / 2, WEB_MOST_BYTES).unwrap();
        assert!(just.starts_with("This WAV is 120.5 MB,"), "{just}");
    }

    fn mono(samples: &[i16]) -> Vec<u8> {
        wav::mono_pcm16(samples, 44_100).unwrap()
    }

    /// A mono WAV of `encoding` holding `samples`, given on that encoding's own scale
    /// (integers for PCM, -1.0..1.0 for float).
    fn mono_as(encoding: Encoding, samples: &[f64]) -> Vec<u8> {
        let full = match encoding {
            Encoding::Int { bits } => (1i64 << (bits - 1)) as f64,
            Encoding::Float { .. } => 1.0,
        };
        let audio = Audio {
            rate: 44_100,
            channels: 1,
            samples: samples.iter().map(|s| (s / full) as f32).collect(),
        };
        wav::write(&audio, encoding).unwrap()
    }

    /// Every integer sample of a PCM WAV, on its own scale.
    fn ints_of(bytes: &[u8]) -> Vec<i32> {
        let layout = wav::layout(bytes).unwrap();
        let Encoding::Int { bits } = layout.format.encoding else {
            panic!("not PCM");
        };
        ints(bytes, layout.data, bits).collect()
    }

    fn gained(bytes: &[u8], db: &str) -> Result<Vec<u8>, String> {
        apply(bytes, bytes, &[(GAIN.to_string(), db.to_string())]).map(|(out, _)| out)
    }

    /// Samples clipped scaling `bytes` by `db`.
    fn clipped_samples(bytes: &[u8], db: f64) -> usize {
        let layout = wav::layout(bytes).unwrap();
        let mut data = bytes[layout.data].to_vec();
        gain_in_place(&mut data, layout.format.encoding, factor(db).unwrap())
    }

    /// A library holding `bytes` as a saved WAV, and its id.
    fn library(bytes: &[u8]) -> (crate::workspace::Workspace, u64) {
        let mut log = crate::log::Log::default();
        let mut workspace = crate::workspace::Workspace::new(crate::testing::context());
        let origin = crate::workspace::Origin::Fresh;
        let id = workspace.ingest("hit.wav".into(), origin, bytes.to_vec(), &mut log);
        (workspace, id)
    }

    /// The document's state on `bytes`, as it opens on them.
    fn opened(bytes: &[u8]) -> State {
        let (workspace, id) = library(bytes);
        State::new(workspace.get(id).unwrap())
    }

    /// Put `bytes` in the working copy of `id`, unsaved.
    fn edited(workspace: &mut crate::workspace::Workspace, id: u64, bytes: Vec<u8>) {
        workspace.replace_bytes(id, bytes, &mut crate::log::Log::default());
        assert!(workspace.get(id).unwrap().is_unsaved());
    }

    /// A 16-bit take of decaying 440 Hz notes, one every 2 s from 0.5 s, peaking at each
    /// of `peaks` (of full scale) in turn.
    fn take(peaks: &[f64]) -> Vec<u8> {
        let rate = 44_100.0;
        let mut samples = vec![0.0; (rate * (0.5 + 2.0 * peaks.len() as f64)) as usize];
        for (k, peak) in peaks.iter().enumerate() {
            let from = (rate * (0.5 + 2.0 * k as f64)) as usize;
            for (i, sample) in samples[from..from + rate as usize].iter_mut().enumerate() {
                let t = i as f64 / rate;
                *sample = peak * (std::f64::consts::TAU * 440.0 * t).sin() * (-t / 0.3).exp();
            }
        }
        let samples: Vec<f64> = samples.iter().map(|s| (s * 32_767.0).round()).collect();
        mono_as(Encoding::Int { bits: 16 }, &samples)
    }

    #[test]
    fn six_db_doubles_a_quiet_sample() {
        let out = gained(&mono(&[1000, -1000, 0]), "6").unwrap();
        let samples = wav::read_pcm16(&out).unwrap().samples;
        // 10^(6/20) = 1.995, so 1000 becomes 1995.
        assert_eq!(samples, [1995, -1995, 0]);
    }

    #[test]
    fn a_gain_clips_at_the_range_of_16_bits() {
        let bytes = mono(&[20_000, -20_000, 100, i16::MIN]);
        assert_eq!(
            wav::read_pcm16(&gained(&bytes, "6.0").unwrap())
                .unwrap()
                .samples,
            [i16::MAX, i16::MIN, 200, i16::MIN]
        );
        assert_eq!(clipped_samples(&bytes, 6.0), 3);
    }

    #[test]
    fn a_second_gain_replaces_the_first_instead_of_stacking_on_it() {
        let bytes = mono(&[1000, 20_000]);
        let twelve = gained(&bytes, "12").unwrap();
        let sets = [
            (GAIN.to_string(), "10".to_string()),
            (GAIN.to_string(), "12".to_string()),
        ];
        let (out, db) = apply(&bytes, &bytes, &sets).unwrap();
        assert_eq!(db, Some(12.0));
        assert!(out == twelve, "10 then 12 dB is 12 dB in all");
        let (back, _) = apply(&bytes, &bytes, &[(GAIN.to_string(), "0".to_string())]).unwrap();
        assert!(
            back == bytes,
            "0 dB is the original, clipping and all undone"
        );
    }

    #[test]
    fn a_gain_changes_only_the_samples_of_the_data_chunk() {
        let mut bytes = mono(&[1000, 2000]);
        let list: &[u8] = b"LIST\x04\x00\x00\x00abcd";
        bytes.splice(36..36, list.iter().copied());
        let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) + list.len() as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        let data = wav::layout(&bytes).unwrap().data;

        let out = gained(&bytes, "-6").unwrap();
        assert_eq!(out.len(), bytes.len());
        let changed: Vec<usize> = (0..bytes.len())
            .filter(|at| out[*at] != bytes[*at])
            .collect();
        assert!(!changed.is_empty());
        assert!(
            changed.iter().all(|at| data.contains(at)),
            "bytes outside {data:?} changed: {changed:?}"
        );
    }

    #[test]
    fn a_gain_the_control_does_not_offer_is_refused() {
        let bytes = mono(&[1000]);
        assert!(gained(&bytes, "25").unwrap_err().contains("runs from"));
        assert!(gained(&bytes, "NaN").is_err());
        assert!(gained(&bytes, "loud").unwrap_err().contains("not a number"));
        let other = apply(&bytes, &bytes, &[("name".to_string(), "x".to_string())]);
        assert!(other.unwrap_err().contains("its cuts"));
    }

    #[test]
    fn a_wav_that_does_not_read_takes_no_gain() {
        let mut bytes = mono(&[1000]);
        bytes[34] = 12;
        assert!(gained(&bytes, "6").unwrap_err().contains("12-bit"));
    }

    #[test]
    fn a_gain_keeps_a_24_bit_file_24_bit_and_clips_at_its_own_range() {
        let top = 8_388_607;
        let bytes = mono_as(
            Encoding::Int { bits: 24 },
            &[1000.0, -1000.0, 5_000_000.0, -8_388_608.0, 1.0],
        );
        let out = gained(&bytes, "6").unwrap();
        assert_eq!(out.len(), bytes.len());
        assert_eq!(
            wav::layout(&out).unwrap().format.encoding,
            Encoding::Int { bits: 24 }
        );
        // 10^(6/20) = 1.995: below 16 bits' step the low sample still keeps its own
        // value, 1 becoming 2, where rounding to 16 bits would have made it 0.
        assert_eq!(ints_of(&out), [1995, -1995, top, -8_388_608, 2]);
        assert_eq!(clipped_samples(&bytes, 6.0), 2);
    }

    #[test]
    fn a_gain_scales_every_integer_width_in_its_own_encoding() {
        for bits in [8, 16, 24, 32] {
            let encoding = Encoding::Int { bits };
            let bytes = mono_as(encoding, &[-50.0, 0.0, 50.0]);
            let out = gained(&bytes, "6").unwrap();
            assert_eq!(wav::layout(&out).unwrap().format.encoding, encoding);
            assert_eq!(ints_of(&out), [-100, 0, 100], "{bits}-bit");
            assert_eq!(clipped_samples(&bytes, 6.0), 0, "{bits}-bit");
        }
    }

    #[test]
    fn a_gain_on_a_float_file_stays_float_and_clips_nothing() {
        for bits in [32, 64] {
            let encoding = Encoding::Float { bits };
            let bytes = mono_as(encoding, &[0.25, -0.75, 0.0]);
            let out = gained(&bytes, "6").unwrap();
            assert_eq!(wav::layout(&out).unwrap().format.encoding, encoding);
            let samples = wav::read(&out).unwrap().samples;
            let factor = 10f32.powf(0.3);
            for (got, was) in samples.iter().zip([0.25f32, -0.75, 0.0]) {
                assert!((got - was * factor).abs() < 1e-6, "{bits}: {got} vs {was}");
            }
            // -0.75 at +6 dB is past full scale: float keeps it.
            assert!(samples[1] < -1.0);
            assert_eq!(clipped_samples(&bytes, 6.0), 0);
            assert_eq!(opened(&bytes).clipping(24.0), Ok(None));
        }
    }

    #[test]
    fn a_24_bit_and_a_float_file_read_as_float_for_the_waveform_and_player() {
        for encoding in [Encoding::Int { bits: 24 }, Encoding::Float { bits: 32 }] {
            let full = match encoding {
                Encoding::Int { .. } => 8_388_608.0,
                Encoding::Float { .. } => 1.0,
            };
            let bytes = mono_as(encoding, &[0.5 * full, -0.25 * full]);
            let state = opened(&bytes);
            assert_eq!(state.audio().unwrap().samples, [0.5, -0.25], "{encoding:?}");
        }
    }

    #[test]
    fn the_clipping_a_gain_would_cause_is_counted_in_notes() {
        let state = opened(&take(&[0.9, 0.3, 0.05]));
        assert_eq!(state.notes.peaks.len(), 3);
        assert_eq!(state.clipping(0.0), Ok(None));
        assert_eq!(state.clipping(-12.0), Ok(None));
        // 0.9 x 1.995 is past full scale; 0.3 x 3.98 is too; 0.05 x 15.85 is not.
        assert_eq!(state.clipping(6.0), Ok(Some("1 of 3 notes clips".into())));
        assert_eq!(state.clipping(12.0), Ok(Some("2 of 3 notes clip".into())));
        assert_eq!(state.clipping(24.0), Ok(Some("2 of 3 notes clip".into())));
        assert!(state.clipping(25.0).is_err());
    }

    #[test]
    fn a_note_clips_exactly_where_one_of_its_samples_does() {
        let bytes = take(&[0.9, 0.3, 0.05]);
        let state = opened(&bytes);
        let layout = wav::layout(&bytes).unwrap();
        let rate = 44_100;
        for tenths in (0..=240).step_by(5) {
            let db = f64::from(tenths) / 10.0;
            // Each note on its own, at 0.5 + 2k s.
            let mut clipped = 0;
            for k in 0..3 {
                let from = layout.data.start + 2 * (rate / 2 + 2 * rate * k);
                let mut note = bytes[from..from + 2 * rate].to_vec();
                let encoding = layout.format.encoding;
                clipped += usize::from(gain_in_place(&mut note, encoding, factor(db).unwrap()) > 0);
            }
            assert_eq!(state.notes.clipped(factor(db).unwrap()), clipped, "{db} dB");
        }
    }

    #[test]
    fn a_wav_closed_with_an_unsaved_gain_opens_on_that_gain() {
        let saved = take(&[0.9, 0.3, 0.05]);
        let (mut workspace, id) = library(&saved);
        for db in ["4.0", "-7.5", "12.0"] {
            edited(&mut workspace, id, gained(&saved, db).unwrap());
            let state = State::new(workspace.get(id).unwrap());
            assert_eq!(state.gain, db.parse::<f64>().unwrap(), "{db}");
            assert!(
                state.original() == &saved[..],
                "{db}: gains start from the saved file"
            );
            assert_eq!(
                state.clipping(state.gain),
                opened(&saved).clipping(state.gain),
                "{db}"
            );
        }
        let state = State::new(workspace.get(id).unwrap());
        assert_eq!(state.clipping(12.0), Ok(Some("2 of 3 notes clip".into())));
    }

    #[test]
    fn a_working_copy_that_is_no_gain_of_the_saved_file_is_its_own_original() {
        let saved = take(&[0.9, 0.3, 0.05]);
        let (mut workspace, id) = library(&saved);
        let other = take(&[0.5, 0.5, 0.5]);
        edited(&mut workspace, id, other.clone());
        let state = State::new(workspace.get(id).unwrap());
        assert_eq!(state.gain, 0.0);
        assert!(state.original() == &other[..]);
    }

    #[test]
    fn saving_a_gain_makes_the_saved_file_the_new_original() {
        let saved = take(&[0.9, 0.3, 0.05]);
        let (mut workspace, id) = library(&saved);
        let mut state = State::new(workspace.get(id).unwrap());
        let (out, db) = apply(
            state.original(),
            state.original(),
            &[(GAIN.to_string(), "6.0".into())],
        )
        .unwrap();
        edited(&mut workspace, id, out.clone());
        state.made(workspace.get(id).unwrap().stamp, db.unwrap());
        state.follow(workspace.get(id).unwrap());
        assert_eq!(state.gain, 6.0);

        workspace.mark_saved(id);
        state.follow(workspace.get(id).unwrap());
        assert_eq!(state.gain, 0.0);
        assert!(state.original() == &out[..]);
    }

    /// `take(peaks)` with an attack and release marked on its first note, and a loop.
    fn marked_take(peaks: &[f64]) -> Vec<u8> {
        let region = |start: u32, frames: u32, label: &str| wav::Region {
            start,
            frames,
            label: label.into(),
        };
        let marks = wav::Marks {
            regions: vec![
                region(22_050, 30_000, "069-b0-v22"),
                region(52_050, 4000, "069-b2-v22"),
            ],
            loops: vec![wav::Loop::forward(40_000, 41_999)],
        };
        wav::with_marks(&take(peaks), &marks).unwrap()
    }

    fn cut(cut: hit::Cut, frame: u32) -> (String, String) {
        (cut_path("069-b0-v22", cut), frame.to_string())
    }

    #[test]
    fn a_moved_cut_is_written_to_the_marks_and_changes_no_sample() {
        let bytes = marked_take(&[0.9, 0.3]);
        let sets = [
            cut(hit::Cut::AttackEnd, 51_000),
            cut(hit::Cut::LoopStart, 39_000),
        ];
        let (out, db) = apply(&bytes, &bytes, &sets).unwrap();
        assert_eq!(db, None);
        let marks = wav::read_marks(&out).unwrap();
        assert_eq!(marks.regions[0].frames, 51_000 - 22_050);
        assert_eq!((marks.loops[0].start, marks.loops[0].end), (39_000, 41_999));
        assert_eq!(
            marks.regions[1],
            wav::read_marks(&bytes).unwrap().regions[1]
        );
        let data = wav::layout(&bytes).unwrap().data;
        assert!(out[wav::layout(&out).unwrap().data] == bytes[data]);

        let past = apply(&bytes, &bytes, &[cut(hit::Cut::LoopEnd, 60_000)]);
        assert!(
            past.unwrap_err().contains("loop end"),
            "past the attack's end"
        );
        let none = (format!("{CUT}070-b0-v22/attack-end"), "100".to_string());
        assert!(apply(&bytes, &bytes, &[none]).is_err(), "no such hit");
    }

    /// A sample, key-off and loop are added and deleted in the marks, the audio
    /// untouched, and each is named for Undo.
    #[test]
    fn pieces_are_added_and_deleted_in_the_marks() {
        let bytes = marked_take(&[0.9, 0.3]);
        let set = |path: String, value: &str| (path, value.to_string());
        let sets = [
            set(format!("{ADDED}071-b0-v00"), "60000..70000"),
            set(format!("{ADDED}071-b2-v00"), "70000..72000"),
            set(format!("{LOOPED}071-b0-v00"), "64000..65000"),
        ];
        let (out, _) = apply(&bytes, &bytes, &sets).unwrap();
        let marks = wav::read_marks(&out).unwrap();
        assert_eq!(marks.regions.len(), 4);
        assert!(marks
            .loops
            .iter()
            .any(|found| (found.start, found.end) == (64_000, 64_999)));
        let data = wav::layout(&bytes).unwrap().data;
        assert!(out[wav::layout(&out).unwrap().data] == bytes[data]);
        let step = Step::before(&bytes, 0.0, &sets);
        assert_eq!(step.what, "add B4 v00, add B4 v00 key-off, add B4 v00 loop");

        let twice = apply(&out, &out, &[set(format!("{ADDED}071-b0-v00"), "1..2")]);
        assert!(twice.is_err(), "a label the take marks already");
        let outside = apply(&out, &out, &[set(format!("{LOOPED}069-b0-v22"), "10..20")]);
        assert!(outside.is_err(), "a loop outside its attack");

        let delete = |piece: &str| set(format!("{DELETED}069-b0-v22/{piece}"), "");
        let (keyless, _) = apply(&out, &out, &[delete("key-off")]).unwrap();
        let marks = wav::read_marks(&keyless).unwrap();
        assert!(!marks
            .regions
            .iter()
            .any(|region| region.label == "069-b2-v22"));
        assert_eq!(marks.loops.len(), 2);
        let (loopless, _) = apply(&out, &out, &[delete("loop")]).unwrap();
        assert_eq!(wav::read_marks(&loopless).unwrap().loops.len(), 1);
        let (gone, _) = apply(&out, &out, &[delete("note")]).unwrap();
        let marks = wav::read_marks(&gone).unwrap();
        assert!(marks
            .regions
            .iter()
            .all(|region| !region.label.starts_with("069")));
        assert_eq!(marks.loops.len(), 1, "the other sample's loop stays");
        assert_eq!(
            Step::before(&out, 0.0, &[delete("note"), delete("key-off")]).what,
            "delete A4 v22, delete A4 v22 key-off"
        );
    }

    /// A rename relabels every region of the sample, keeps its loop, and refuses a name
    /// the take marks already.
    #[test]
    fn a_sample_is_renamed_every_region_of_it() {
        let bytes = marked_take(&[0.9, 0.3]);
        let rename = |to: &str| [(format!("{RENAMED}069-b0-v22"), to.to_string())];
        let (out, _) = apply(&bytes, &bytes, &rename("071-b0-v30")).unwrap();
        let marks = wav::read_marks(&out).unwrap();
        let labels: Vec<&str> = marks
            .regions
            .iter()
            .map(|region| region.label.as_str())
            .collect();
        assert_eq!(labels, ["071-b0-v30", "071-b2-v30"]);
        assert_eq!(marks.loops.len(), 1);
        assert_eq!(
            Step::before(&bytes, 0.0, &rename("071-b0-v30")).what,
            "rename A4 v22 to B4 v30"
        );
        let added = [(format!("{ADDED}071-b0-v30"), "60000..70000".to_string())];
        let (both, _) = apply(&bytes, &bytes, &added).unwrap();
        assert!(apply(&both, &both, &rename("071-b0-v30")).is_err());

        assert_eq!(
            hit::named("B4 v30"),
            Ok((71, nord_format::formats::npno::encode::LayerTag::Value(30)))
        );
        assert_eq!(
            hit::named("c4 l01"),
            Ok((60, nord_format::formats::npno::encode::LayerTag::Index(1)))
        );
        assert!(hit::named("B4").is_err() && hit::named("H4 v30").is_err());
    }

    #[test]
    fn a_gain_keeps_the_cuts_moved_before_it() {
        let saved = marked_take(&[0.9, 0.3]);
        let (moved, _) = apply(&saved, &saved, &[cut(hit::Cut::ReleaseEnd, 58_000)]).unwrap();
        let (out, db) = apply(&saved, &moved, &[(GAIN.to_string(), "-6.0".into())]).unwrap();
        assert_eq!(db, Some(-6.0));
        assert_eq!(
            wav::read_marks(&out).unwrap(),
            wav::read_marks(&moved).unwrap()
        );
        assert!(
            wav::read(&out).unwrap().samples
                == wav::read(&gained(&saved, "-6.0").unwrap()).unwrap().samples
        );

        // Closed unsaved and opened again: the gain is found, and the saved file is still
        // what it is applied to.
        let (mut workspace, id) = library(&saved);
        edited(&mut workspace, id, out);
        let state = State::new(workspace.get(id).unwrap());
        assert_eq!(state.gain, -6.0);
        assert!(state.original() == &saved[..]);
    }

    #[test]
    fn a_file_with_no_onset_is_one_note() {
        let state = opened(&mono(&[20_000, -20_000, 100]));
        assert_eq!(state.notes.peaks, [(-20_000, 20_000)]);
        assert_eq!(state.clipping(6.0), Ok(Some("it clips".into())));
    }

    #[test]
    fn the_header_states_the_rate_channels_format_and_length() {
        let stated = |bytes: &[u8]| facts(&wav::layout(bytes).unwrap());
        assert_eq!(
            stated(&mono(&[0; 44_100])),
            "44100 Hz · mono · 16-bit · 1.000 s"
        );
        let stereo = wav::write(
            &Audio {
                rate: 48_000,
                channels: 2,
                samples: vec![0.0; 144_000],
            },
            Encoding::Float { bits: 32 },
        )
        .unwrap();
        assert_eq!(
            stated(&stereo),
            "48000 Hz · stereo · 32-bit float · 1.500 s"
        );
        let six = wav::write(
            &Audio {
                rate: 44_100,
                channels: 6,
                samples: Vec::new(),
            },
            Encoding::Int { bits: 24 },
        )
        .unwrap();
        assert_eq!(stated(&six), "44100 Hz · 6 channels · 24-bit · 0.000 s");
    }

    #[test]
    fn a_wav_is_a_take_once_it_marks_a_stroke_region() {
        let raw = mono(&[0; 1000]);
        let marked = |label: &str| {
            let region = wav::Region {
                start: 0,
                frames: 500,
                label: label.into(),
            };
            let marks = wav::Marks {
                regions: vec![region],
                loops: Vec::new(),
            };
            wav::with_marks(&raw, &marks).unwrap()
        };
        assert!(!is_take(&raw), "as recorded");
        assert!(!is_take(&marked("intro")), "a region that names no stroke");
        assert!(is_take(&marked("060-b0-v22")));
        assert_eq!(
            crate::strings::kind_word(Kind::Wav, Some(crate::browser::Qualifier::Processed)),
            "WAV (processed)"
        );
    }

    #[test]
    fn only_a_riff_wave_container_is_a_wav_by_its_bytes() {
        assert!(is_wav(&mono(&[0; 8])));
        assert!(!is_wav(b"RIFF"));
        assert!(!is_wav(b"not a wav at all"));
        assert!(!is_wav(&[]));
    }

    #[test]
    fn detect_marks_what_it_found_in_place_of_the_regions_and_undo_names_it() {
        let found = wav::Marks {
            regions: vec![
                wav::Region {
                    start: 100,
                    frames: 900,
                    label: "060-b0-v00".into(),
                },
                wav::Region {
                    start: 1_000,
                    frames: 500,
                    label: "060-b2-v00".into(),
                },
            ],
            loops: vec![wav::Loop::forward(300, 699)],
        };
        let value = detected_value(&found);
        let bare = wav::pcm16(&[0i16; 4_000], 35_002, 1).unwrap();
        let before = wav::Marks {
            regions: vec![wav::Region {
                start: 0,
                frames: 50,
                label: "072-b0-v00".into(),
            }],
            loops: Vec::new(),
        };
        let old = wav::with_marks(&bare, &before).unwrap();
        let sets = [(DETECTED.to_string(), value)];
        let (out, _) = apply(&old, &old, &sets).unwrap();
        assert_eq!(
            wav::read_marks(&out).unwrap(),
            found,
            "the old regions are gone"
        );
        assert_eq!(Step::before(&old, 0.0, &sets).what, "detect");
        assert!(detected_marks("12 x 060-b0-v00").is_err());
    }
}
