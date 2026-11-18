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

mod detected;
mod edits;
mod limits;
#[cfg(test)]
mod tests;

pub(in crate::document) use detected::detect_loud;
use detected::DETECT_ASK_WIDTH;
pub use detected::{Detect, DETECT_REPLACES};
pub(crate) use edits::detected_value;
use edits::*;
pub(crate) use limits::web_refusal;
use limits::*;

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

/// The path prefix of a set that deletes, from the hit whose attack it names, the
/// piece after it: `note`, `loop` or `key-off`.
pub const DELETED: &str = "delete/";

/// The path prefix of a set that renames the hit whose attack it names to the hit
/// whose attack label its value is, every region of it.
pub const RENAMED: &str = "rename/";

/// The path of a set that replaces every region and loop the take marks with what
/// Detect found. Its value holds a line per region, `<start> <frames> <label>`, and a
/// line per loop, `loop <start> <end>`, its end its last frame.
pub const DETECTED: &str = "detect";

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
    wav::read_marks(bytes).is_ok_and(|marks| marks_a_stroke(&marks))
}

/// Whether `marks` label a region as a stroke the build takes: an attack, resonance or
/// release, or an attack's fade.
pub(crate) fn marks_a_stroke(marks: &wav::Marks) -> bool {
    (marks.regions.iter()).any(|region| take::mark(&region.label, 0..0).hit.is_some())
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
            None if is_piece(path) => Some(wav::read_marks(current).map_err(error)?),
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
            return Err(format!("{path}: names no cut of a hit"));
        };
        let frame: u32 = value
            .trim()
            .parse()
            .map_err(|_| format!("{path}: {value} is not a frame"))?;
        let held = match marks.take() {
            Some(held) => held,
            None => wav::read_marks(current).map_err(error)?,
        };
        let frames = wav::layout(current).map_err(error)?.frames();
        let frames = u32::try_from(frames).map_err(|_| {
            format!("{path}: the take's {frames} frames are more than a cue can mark")
        })?;
        marks = Some(hit::moved(&held, hit, cut, frame, frames)?);
    }
    let mut out = match total {
        Some(db) => {
            let mut out = original.to_vec();
            wav::scale(&mut out[layout.data], layout.format.encoding, factor(db)?)
                .map_err(error)?;
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
        refused_by_build(current, marks)?;
        out = wav::with_marks(&out, marks).map_err(error)?;
    }
    if wav::read_settings(&out).ok().as_ref() != Some(&settings) {
        out = wav::with_settings(&out, &settings).map_err(error)?;
    }
    Ok((out, total))
}

/// `entity`'s bytes with `sets` applied, `held` the document open on it where one is, and
/// the total gain they hold against its original. A gain that clips is warned of.
pub fn edited(
    held: Option<&State>,
    entity: &LocalEntity,
    sets: &[(String, String)],
    log: &mut crate::log::Log,
) -> Result<(Vec<u8>, f64), String> {
    let original = held.map_or(&entity.bytes[..], State::original);
    let (out, db) = apply(original, &entity.bytes, sets)?;
    if let (Some(db), Some(Ok(Some(clipped)))) =
        (db, held.zip(db).map(|(held, db)| held.clipping(db)))
    {
        log.warn(format!("{}: at {db:+.1} dB {clipped}", entity.name));
    }
    // An edit that sets no gain keeps the one the bytes hold.
    Ok((out, db.unwrap_or_else(|| held.map_or(0.0, State::applied))))
}

/// Refuse `marks` for the take in `current` where the build would refuse them, as
/// [`nord_format::take::build::cuts`] checks, but would build the take as it is: an edit
/// never breaks a take that builds, and a take that does not yet can still be mended.
fn refused_by_build(current: &[u8], marks: &wav::Marks) -> Result<(), String> {
    let frames = wav::layout(current).map_err(|e| e.to_string())?.frames();
    let builds = |marks: &wav::Marks| nord_format::take::build::cuts("", marks, frames).map(|_| ());
    let Err(why) = builds(marks) else {
        return Ok(());
    };
    let before = wav::read_marks(current).map_err(|e| e.to_string())?;
    match builds(&before) {
        Ok(_) => Err(format!(
            "the build would refuse this: {}",
            why.to_string().trim_start_matches(": ")
        )),
        Err(_) => Ok(()),
    }
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
            hint: "the hits, regions and loops the file marks; a raw take, as recorded, has none",
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
    /// The total gain the audio `read` holds against `original`.
    read_gain: f64,
    /// The file as last saved, which Revert goes back to; or, where those bytes cannot
    /// be had or the working copy is not a gain of them, the working copy itself.
    original: Bytes,
    /// The stamps of the bytes made here from `original`, and the gain each holds, so a
    /// revert to one of them shows its gain again.
    made: Vec<(u64, f64)>,
    /// The stamp of the saved file `original` was taken against, so a save of any edit,
    /// a gain or not, makes the saved file the new original.
    saved_at: u64,
    notes: Notes,
    read: Read,
    /// What the take's lane shows, kept across new bytes.
    pub view: take::View,
    /// What the picked hit's lane shows, kept across new bytes.
    pub close: hit::Close,
    /// Both lanes as they were before this frame's menu edit moved the pick, put back if
    /// the edit is refused, so the pick never names a hit the take does not mark.
    before_edit: Option<(take::View, hit::Close)>,
    /// What the take is heard as, kept across new bytes.
    pub audition: audition::Audition,
    /// What Detect asks for and found, kept across new bytes.
    pub detect: Detect,
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
        let Ok(samples) = wav::int_samples(bytes, layout.data, bits) else {
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
        for (at, sample) in samples.enumerate() {
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
        let clips = |sample: i32| wav::scaled(sample, factor, self.bits).1;
        self.peaks
            .iter()
            .filter(|(low, high)| clips(*low) || clips(*high))
            .count()
    }
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
    /// The hits `marked` marks, in take order, for the arrows that step through them.
    order: Vec<take::Hit>,
    /// What the take states about its build, nothing where it states nothing or its
    /// settings do not read.
    settings: wav::Settings,
    summary: take::Summary,
    drawn: take::Drawn,
    /// The envelopes last drawn on the hit's lane.
    close: take::Drawn,
}

/// A read of nothing, only to stand in while a read is moved out.
impl Default for Read {
    fn default() -> Read {
        Read {
            stamp: 0,
            source: Err(String::new()),
            too_large: false,
            marks: Err(String::new()),
            marked: Err(String::new()),
            order: Vec::new(),
            settings: wav::Settings::default(),
            summary: take::Summary::default(),
            drawn: take::Drawn::default(),
            close: take::Drawn::default(),
        }
    }
}

impl Read {
    /// `entity`'s bytes read as `old` read its audio: the marks and settings read again,
    /// the audio, its summary and the lanes drawn of it kept.
    fn again(entity: &LocalEntity, old: Read) -> Read {
        let marks = wav::read_marks(&entity.bytes).map_err(|e| e.to_string());
        let marked = marks
            .as_ref()
            .map(take::Marked::from_marks)
            .map_err(Clone::clone);
        Read {
            stamp: entity.stamp,
            order: marked.as_ref().map(take::Marked::order).unwrap_or_default(),
            marked,
            marks,
            settings: wav::read_settings(&entity.bytes).unwrap_or_default(),
            ..old
        }
    }

    fn of(entity: &LocalEntity) -> Read {
        let refused = web_refusal(entity.bytes.len());
        let too_large = refused.is_some();
        let source = match refused {
            Some(why) => Err(why),
            None => wav::read(&entity.bytes)
                .map_err(|e| e.to_string())
                .and_then(heard_rate),
        };
        let marks = wav::read_marks(&entity.bytes).map_err(|e| e.to_string());
        let marked = marks
            .as_ref()
            .map(take::Marked::from_marks)
            .map_err(Clone::clone);
        Read {
            stamp: entity.stamp,
            too_large,
            order: marked.as_ref().map(take::Marked::order).unwrap_or_default(),
            summary: source.as_ref().map(take::Summary::of).unwrap_or_default(),
            source,
            marked,
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
/// Measured on the loudest sample the gain left unclipped, then proved: the original's
/// samples at that gain must be the edited file's exactly, so an edit that is not a gain,
/// or a gain off the control's steps, is never taken for one. Only the samples are
/// proved: the marks and settings a gain carries over may sit in other chunks, in
/// whatever order the edits wrote them.
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
    let (from, to) = (wav::layout(was).ok()?, wav::layout(now).ok()?);
    if from.format.encoding != to.format.encoding {
        return None;
    }
    let mut data = was[from.data].to_vec();
    wav::scale(&mut data, from.format.encoding, factor(db).ok()?).ok()?;
    (data[..] == now[to.data]).then_some(db)
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
            read_gain: applied,
            original,
            made: vec![(entity.stamp, applied)],
            saved_at: entity.saved.stamp(),
            notes,
            read,
            view: take::View::default(),
            close: hit::Close::default(),
            before_edit: None,
            audition: audition::Audition::default(),
            detect: Detect::default(),
        }
    }

    /// Read the bytes again where they are no longer the ones read. Bytes this document
    /// made keep the original and show their gain. Any others, and saved bytes, which
    /// the next gain starts from, open the document again.
    pub fn follow(&mut self, entity: &LocalEntity) {
        let saved = !entity.is_unsaved() && entity.saved.stamp() != self.saved_at;
        if self.read.stamp == entity.stamp && !saved {
            return;
        }
        match self.made.iter().find(|(stamp, _)| *stamp == entity.stamp) {
            Some(&(_, total)) if !saved => {
                // Bytes made here at the gain the audio already holds differ from it in
                // their marks and settings only, so the audio read is kept, not decoded
                // again for each moved cut.
                let same_audio = total == self.read_gain;
                self.read_gain = total;
                self.applied = total;
                self.gain = total;
                self.read = match same_audio {
                    true => Read::again(entity, std::mem::take(&mut self.read)),
                    false => Read::of(entity),
                };
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

    /// Put both lanes back as they were before this frame's menu edit, which was refused.
    pub fn refused(&mut self) {
        if let Some((view, close)) = self.before_edit.take() {
            (self.view, self.close) = (view, close);
        }
    }

    /// Note that the bytes stamped `stamp` hold `total` against the original.
    pub fn made(&mut self, stamp: u64, total: f64) {
        self.made.retain(|(made, _)| *made != stamp);
        self.made.push((stamp, total));
        self.applied = total;
    }

    /// What Play plays: the picked hit as recorded, from its first cut to its last;
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

    /// The picked hit's cuts, where one is picked in marks that read.
    pub fn cuts(&self) -> Option<hit::Cuts> {
        let marked = self.read.marked.as_ref().ok()?;
        Some(hit::Cuts::of(marked, self.view.picked?))
    }

    /// Start finding the take's noise where it is cleaned, or take it once found.
    pub fn follow_noise(&mut self, ctx: &egui::Context) {
        let clean = self.cleaned();
        if let Ok(audio) = &self.read.source {
            self.audition.follow(audio, clean, ctx);
        }
    }

    /// Make the picked hit ready to play, again only where the bytes, the pick, the
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
        let sounding = match self.audition.cleaning(audio, self.cleaned()) {
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

    /// How the take's noise floor is read where it is cleaned, as prep read it.
    pub fn cleaned(&self) -> Option<nord_format::take::detect::Floor> {
        self.clean()
            .then(|| self.read.settings.floor.unwrap_or_default())
    }

    /// The picked hit as the instrument will play it, heard as the take's kind, and
    /// cleaned of `noise` where it is given.
    ///
    /// ⚠️ Shapes the hit's audio: run when it is asked to play, not per frame.
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

    /// Whether the file marks hits, so the take's cards show.
    pub fn marks_samples(&self) -> bool {
        self.read.marks.as_ref().is_ok_and(marks_a_stroke)
    }

    /// Whether a library can be built from the take: it marks hits and its audio
    /// reads, which on the web a take too large for the browser does not.
    pub fn builds(&self) -> bool {
        self.marks_samples() && self.read.source.is_ok()
    }

    /// Start Detect on the take, where its audio reads: at once on a take that marks no
    /// region, and on one that does once the user confirms it may replace them.
    pub fn start_detect(&mut self, ctx: &egui::Context) {
        let marks_regions = (self.read.marks.as_ref()).is_ok_and(|marks| !marks.regions.is_empty());
        match marks_regions {
            true => self.detect.asking = true,
            false => self.detect_now(ctx),
        }
    }

    fn detect_now(&mut self, ctx: &egui::Context) {
        if let Ok(audio) = &self.read.source {
            self.detect.start(ctx, audio);
        }
    }

    /// While Detect waits for the user to confirm it may replace the take's regions, ask,
    /// and start it on Confirm.
    pub fn confirm_detect(&mut self, ctx: &egui::Context) {
        if !self.detect.asking {
            return;
        }
        let mut answer = None;
        egui::Modal::new(egui::Id::new("detect_confirm"))
            .frame(crate::sheet::frame(&ctx.style().visuals))
            .show(ctx, |ui| {
                ui.set_width(crate::sheet::width(ui.ctx(), DETECT_ASK_WIDTH));
                ui.add_space(crate::sheet::GAP * 4.0);
                crate::sheet::section(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(DETECT_REPLACES).strong()).wrap());
                });
                crate::sheet::foot(
                    ui,
                    |_| {},
                    |ui| {
                        // Right to left: Confirm at the far right, Cancel at the left.
                        if crate::sheet::primary(ui, None, "Confirm").clicked() {
                            answer = Some(true);
                        }
                        if crate::sheet::secondary(ui, None, "Cancel").clicked() {
                            answer = Some(false);
                        }
                    },
                );
                if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    answer = Some(false);
                }
            });
        if let Some(confirmed) = answer {
            self.detect.asking = false;
            if confirmed {
                self.detect_now(ctx);
            }
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
            (1, _) => Some(format!("1 of {notes} strikes clips")),
            _ => Some(format!("{clipped} of {notes} strikes clip")),
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
    state.before_edit = None;
    let audio = state.read.source.as_ref().ok();
    state.detect.follow(sets, audio);
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
    let read = &mut state.read;
    match &read.source {
        Ok(audio) => {
            let picked = state.view.picked;
            let place = state.view.place;
            // Both lanes draw a dragged cut where it is now; the file moves it when the
            // drag ends.
            // A take read holds no more frames than a u32 counts, as `heard_rate` asks.
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
            );
            if wanted.is_some() {
                state.before_edit = Some((state.view.clone(), state.close.clone()));
            }
            if let (Some(hit::Wanted::Rename(to)), Some(from), Ok(held)) =
                (wanted, picked, &read.marked)
            {
                let from = hit::Cuts::of(held, from);
                sets.push((format!("{RENAMED}{}", from.label), adding::attack_label(to)));
                state.view.picked = Some(to);
                state.close.renamed(to);
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
                (false, true) => (PLAY_ORIGINAL, Glyph::AudioLines),
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

/// The sets that do what the close lane's menus asked of the picked hit, or of the
/// place clicked: a hit added there is picked, and a hit deleted leaves its place.
fn pieces(
    wanted: hit::Wanted,
    audio: &Audio,
    marked: &take::Marked,
    view: &mut take::View,
    sets: &mut Sets,
) {
    use hit::{Piece, Wanted};
    if let (Wanted::Add(Piece::Hit), Some(place)) = (wanted, view.place) {
        let added = adding::hit(audio, marked, place);
        sets.push((format!("{ADDED}{}", added.label), span_value(&added.attack)));
        if let Some(release) = &added.release {
            sets.push((
                format!("{ADDED}{}", adding::release_label(added.hit)),
                span_value(release),
            ));
        }
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
        Wanted::Add(Piece::Release) => {
            if let Some(release) = adding::release(audio, attack.start, attack.end) {
                sets.push((
                    format!("{ADDED}{}", adding::release_label(hit)),
                    span_value(&release),
                ));
            }
        }
        Wanted::Add(Piece::Loop) => sets.push((
            format!("{LOOPED}{}", cuts.label),
            span_value(&adding::default_loop(&attack, audio.rate)),
        )),
        Wanted::Add(Piece::Hit) | Wanted::Rename(_) => {}
        Wanted::Delete(piece) => {
            sets.push((
                format!("{DELETED}{}/{}", cuts.label, piece_slug(piece)),
                String::new(),
            ));
            if piece == Piece::Hit {
                (view.picked, view.place) = (None, Some(attack.start));
            }
        }
    }
}

/// What the arrow that picks the hit before the picked one says, on hover.
pub const BEFORE: &str = "Previous hit";

/// What the arrow that picks the hit after the picked one says, on hover.
pub const AFTER: &str = "Next hit";

/// The small arrows at the far right of the row under the picked hit, which pick the
/// hit before and after it in the take.
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
            let next = state.view.beside(&state.read.order, forward);
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

/// The label of Play while a hit is picked: it plays that hit as the take holds it,
/// from its first cut to its last, unshaped.
pub const PLAY_ORIGINAL: &str = "Play original";

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
