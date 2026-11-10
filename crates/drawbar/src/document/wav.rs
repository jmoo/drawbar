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
use super::{hit, sample, take};
use crate::app;
use crate::browser::Kind;
use crate::icon::Glyph;
use crate::workspace::{Bytes, LocalEntity};

/// The extension a WAV carries, and the tag it is kept under.
pub const EXTENSION: &str = "wav";

/// The path of a WAV's one edit: its total gain in dB against the file as last saved.
pub const GAIN: &str = "gain";

/// The gains the control offers, in dB.
const GAINS: RangeInclusive<f64> = -24.0..=24.0;

/// Whether these bytes are in a RIFF/WAVE container.
///
/// Tests the container only: a compressed WAV still opens as one, and its document says
/// why it does not read.
pub fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE"
}

/// Whether these bytes mark a region labelled as a stroke: a take `nord piano prep` has
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

/// `original` at the last `gain` set, and that gain in dB.
///
/// ⚠️ A gain is a total, never a step: each one is applied to `original`, so a second
/// gain replaces the first instead of stacking on it, and nothing the first clipped or
/// rounded is carried into the second.
pub fn apply(original: &[u8], sets: &[(String, String)]) -> Result<(Vec<u8>, f64), String> {
    let layout = wav::layout(original).map_err(|e| e.to_string())?;
    let mut total = None;
    for (path, value) in sets {
        if path != GAIN {
            return Err(format!("{path}: a WAV's only edit is its {GAIN}"));
        }
        let db: f64 = value
            .trim()
            .parse()
            .map_err(|_| format!("{GAIN}: {value} is not a number of dB"))?;
        factor(db)?;
        total = Some(db);
    }
    let mut out = original.to_vec();
    let db = total.unwrap_or(0.0);
    gain_in_place(&mut out[layout.data], layout.format.encoding, factor(db)?);
    Ok((out, db))
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
            hint: "the rate, channels, sample format and length the file declares",
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
    /// The file as last saved, which Revert goes back to; or, where those bytes cannot
    /// be had or the working copy is not a gain of them, the working copy itself.
    original: Bytes,
    /// The stamps of the bytes made here from `original`, and the gain each holds, so a
    /// revert to one of them shows its gain again.
    made: Vec<(u64, f64)>,
    notes: Notes,
    read: Read,
    /// What the take's lane shows, kept across new bytes.
    pub view: take::View,
    /// What the picked hit's lane shows, kept across new bytes.
    pub close: hit::Close,
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

/// One set of a WAV's bytes, read.
///
/// ⚠️ Reading copies every sample, so it happens when the bytes change and never per
/// frame.
struct Read {
    /// The [`LocalEntity::stamp`] of the bytes read.
    stamp: u64,
    /// The audio as float, or why the bytes do not read.
    source: Result<Audio, String>,
    /// The regions and loops the file marks, or why they do not read.
    marked: Result<take::Marked, String>,
    summary: take::Summary,
    drawn: take::Drawn,
    /// The envelopes last drawn on the hit's lane.
    close: take::Drawn,
}

impl Read {
    fn of(entity: &LocalEntity) -> Read {
        let source = wav::read(&entity.bytes).map_err(|e| e.to_string());
        Read {
            stamp: entity.stamp,
            summary: source.as_ref().map(take::Summary::of).unwrap_or_default(),
            source,
            marked: take::Marked::of(&entity.bytes),
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
    let (made, _) = apply(was, &[(GAIN.to_string(), format!("{db:.1}"))]).ok()?;
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
            notes,
            read,
            view: take::View::default(),
            close: hit::Close::default(),
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
                let (view, close) = (
                    std::mem::take(&mut self.view),
                    std::mem::take(&mut self.close),
                );
                *self = State::new(entity);
                (self.view, self.close) = (view, close);
            }
        }
    }

    /// The bytes every gain is applied to.
    pub fn original(&self) -> &[u8] {
        &self.original
    }

    /// Note that the bytes stamped `stamp` hold `total` against the original.
    pub fn made(&mut self, stamp: u64, total: f64) {
        self.made.retain(|(made, _)| *made != stamp);
        self.made.push((stamp, total));
        self.applied = total;
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

/// Draw the document: the waveform with Play and the gain over it where the bytes read,
/// and why not where they do not. `true` once the operator has asked to play the file, or
/// to stop it where it is playing.
pub fn ui(ui: &mut egui::Ui, state: &mut State, playing: bool, sets: &mut Sets) -> bool {
    let mut audition = false;
    let read = &mut state.read;
    match &read.source {
        Ok(audio) => {
            take::ui(
                ui,
                audio,
                &read.summary,
                &mut read.drawn,
                &read.marked,
                &mut state.view,
                playing,
            );
            ui.add_space(10.0);
            hit::ui(
                ui,
                audio,
                &read.summary,
                &mut read.close,
                &read.marked,
                state.view.picked,
                &mut state.close,
                playing,
            );
        }
        Err(why) => {
            ui.label(
                egui::RichText::new(format!("This WAV does not read: {why}"))
                    .color(app::bad(ui.visuals())),
            );
        }
    }
    if state.audio().is_some() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (label, glyph) = match playing {
                true => ("Stop", Glyph::X),
                false => ("Play", Glyph::AudioLines),
            };
            audition = sample::action(ui, label, glyph, app::accent(ui.visuals()));
            ui.add_space(12.0);
            gain(ui, state, sets);
        });
    }
    audition
}

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
        apply(bytes, &[(GAIN.to_string(), db.to_string())]).map(|(out, _)| out)
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
        let (out, db) = apply(&bytes, &sets).unwrap();
        assert_eq!(db, 12.0);
        assert!(out == twelve, "10 then 12 dB is 12 dB in all");
        let (back, _) = apply(&bytes, &[(GAIN.to_string(), "0".to_string())]).unwrap();
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
        let other = apply(&bytes, &[("name".to_string(), "x".to_string())]);
        assert!(other.unwrap_err().contains("only edit"));
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
        let (out, db) = apply(state.original(), &[(GAIN.to_string(), "6.0".into())]).unwrap();
        edited(&mut workspace, id, out.clone());
        state.made(workspace.get(id).unwrap().stamp, db);
        state.follow(workspace.get(id).unwrap());
        assert_eq!(state.gain, 6.0);

        workspace.mark_saved(id);
        state.follow(workspace.get(id).unwrap());
        assert_eq!(state.gain, 0.0);
        assert!(state.original() == &out[..]);
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
}
