//! The WAV document: the whole file's waveform, audition through the app's player, a
//! gain edit, and the encode panel.
//!
//! A gain rewrites the samples of the data chunk and nothing else, so every other chunk
//! the file holds survives it.

use std::ops::RangeInclusive;

use eframe::egui;
use nord_format::wav::{self, Pcm16};

use super::controls::Sets;
use super::encode;
use super::header::{Body, Cell};
use super::sample;
use crate::app;
use crate::browser::Kind;
use crate::icon::Glyph;
use crate::workspace::LocalEntity;

/// The extension a WAV carries, and the tag it is kept under.
pub const EXTENSION: &str = "wav";

/// The path of a WAV's one edit: a gain in dB, applied to every sample.
pub const GAIN: &str = "gain";

/// The gains the control offers, in dB.
const GAINS: RangeInclusive<f64> = -24.0..=24.0;

/// Whether these bytes are in a RIFF/WAVE container.
///
/// Tests the container only: a 24-bit WAV still opens as one, and its document says why
/// it does not read.
pub fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE"
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

/// `sample` times `factor`, rounded, and whether it was clipped to fit 16 bits.
fn scaled(sample: i16, factor: f64) -> (i16, bool) {
    let exact = (f64::from(sample) * factor).round();
    let held = exact.clamp(f64::from(i16::MIN), f64::from(i16::MAX));
    (held as i16, held != exact)
}

/// The file with each `gain` set applied in turn to every sample of its data chunk, and
/// how many samples were clipped.
pub fn apply(bytes: &[u8], sets: &[(String, String)]) -> Result<(Vec<u8>, usize), String> {
    let layout = wav::pcm16_layout(bytes).map_err(|e| e.to_string())?;
    let mut out = bytes.to_vec();
    let mut clipped = 0;
    for (path, value) in sets {
        if path != GAIN {
            return Err(format!("{path}: a WAV's only edit is its {GAIN}"));
        }
        let db: f64 = value
            .trim()
            .parse()
            .map_err(|_| format!("{GAIN}: {value} is not a number of dB"))?;
        let factor = factor(db)?;
        for sample in out[layout.data.clone()].chunks_exact_mut(2) {
            let (held, clips) = scaled(i16::from_le_bytes([sample[0], sample[1]]), factor);
            sample.copy_from_slice(&held.to_le_bytes());
            clipped += usize::from(clips);
        }
    }
    Ok((out, clipped))
}

/// The identity cell a WAV puts on the header: the rate, channels and length its
/// container declares, read without copying its samples.
pub fn stated(entity: &LocalEntity) -> Option<Cell> {
    if Kind::of(entity) != Kind::Wav {
        return None;
    }
    let layout = wav::pcm16_layout(&entity.bytes).ok()?;
    Some(Cell {
        label: "Audio",
        body: Body::Read(facts(layout.rate, layout.channels, layout.frames())),
        hint: "the rate, channels and length the file declares",
    })
}

/// `44100 Hz · stereo · 1.500 s`.
fn facts(rate: u32, channels: u16, frames: usize) -> String {
    let channels = match channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n} channels"),
    };
    let seconds = frames as f64 / f64::from(rate.max(1));
    format!("{rate} Hz · {channels} · {seconds:.3} s")
}

/// What a WAV's document keeps between frames: the encode panel's draft, the gain typed
/// so far, and a read of the bytes it is open on.
pub struct State {
    pub draft: encode::Draft,
    /// In dB, to one decimal.
    pub gain: f64,
    read: Read,
}

/// One channel's envelope, as [`sample::channel_envelope`] takes it.
type Lane = Vec<(f32, f32)>;

/// One set of a WAV's bytes, read.
///
/// ⚠️ Reading copies every sample, so it happens when the bytes change and never per
/// frame.
struct Read {
    /// The [`LocalEntity::stamp`] of the bytes read.
    stamp: u64,
    source: encode::Source,
    /// How many samples hold each value, by the value's distance above `i16::MIN`, so a
    /// gain's clipping is counted without going through the samples again.
    census: Vec<u64>,
    /// Each channel's envelope, and the column count it was taken at.
    lanes: Option<(usize, Vec<Lane>)>,
}

impl Read {
    fn of(entity: &LocalEntity) -> Read {
        let source = encode::read(&entity.bytes);
        let mut census = Vec::new();
        if let Ok(pcm) = &source {
            census = vec![0; 1 << 16];
            for sample in &pcm.samples {
                census[usize::from(sample.abs_diff(i16::MIN))] += 1;
            }
        }
        Read {
            stamp: entity.stamp,
            source,
            census,
            lanes: None,
        }
    }

    /// Each channel's envelope across `columns`.
    fn lanes(&mut self, columns: usize) -> Option<&[Lane]> {
        let pcm = self.source.as_ref().ok()?;
        if self.lanes.as_ref().is_none_or(|(at, _)| *at != columns) {
            let lanes = (0..usize::from(pcm.channels))
                .map(|channel| {
                    sample::channel_envelope(&pcm.samples, pcm.channels, Some(channel), columns)
                })
                .collect();
            self.lanes = Some((columns, lanes));
        }
        self.lanes.as_ref().map(|(_, lanes)| lanes.as_slice())
    }

    /// How many samples a gain of `db` would clip.
    fn clipped(&self, db: f64) -> Result<u64, String> {
        let factor = factor(db)?;
        Ok((i16::MIN..=i16::MAX)
            .zip(&self.census)
            .filter(|(sample, _)| scaled(*sample, factor).1)
            .map(|(_, count)| count)
            .sum())
    }
}

impl State {
    pub fn new(entity: &LocalEntity) -> State {
        State {
            draft: encode::Draft::new(&entity.name),
            gain: 0.0,
            read: Read::of(entity),
        }
    }

    /// Read the bytes again where they are no longer the ones read, keeping the draft.
    pub fn follow(&mut self, entity: &LocalEntity) {
        if self.read.stamp != entity.stamp {
            self.read = Read::of(entity);
        }
    }

    /// The audio, or why the bytes do not read as 16-bit PCM.
    pub fn source(&self) -> &encode::Source {
        &self.read.source
    }

    pub fn pcm(&self) -> Option<&Pcm16> {
        self.read.source.as_ref().ok()
    }
}

/// What a WAV's document asks of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Play the file, or stop it where it is playing.
    Audition,
    /// Make an instrument from it, as the panel describes.
    Encode,
}

/// Draw the document: the waveform with Play and the gain over it where the bytes read,
/// and why not where they do not, then the encode panel.
pub fn ui(ui: &mut egui::Ui, state: &mut State, playing: bool, sets: &mut Sets) -> Option<Ask> {
    let mut ask = None;
    let columns = (ui.available_width() * ui.ctx().pixels_per_point()).round() as usize;
    match state.read.lanes(columns) {
        Some(lanes) => {
            for lane in lanes {
                sample::waveform(ui, lane, playing);
                ui.add_space(2.0);
            }
        }
        None => {
            if let Err(why) = &state.read.source {
                ui.label(
                    egui::RichText::new(format!("This WAV does not read: {why}"))
                        .color(app::bad(ui.visuals())),
                );
            }
        }
    }
    if state.pcm().is_some() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (label, glyph) = match playing {
                true => ("Stop", Glyph::X),
                false => ("Play", Glyph::AudioLines),
            };
            if sample::action(ui, label, glyph, app::accent(ui.visuals())) {
                ask = Some(Ask::Audition);
            }
            ui.add_space(12.0);
            gain(ui, state, sets);
        });
    }
    ui.add_space(12.0);
    ui.separator();
    if encode::ui(ui, &mut state.draft, &state.read.source) {
        ask = Some(Ask::Encode);
    }
    ask
}

/// The label of the button that applies the gain.
pub const RESCALE: &str = "Rescale";

/// The gain control, the samples it would clip, and the button that applies it.
///
/// ⚠️ Applied by a click and not as the control moves: each application scales what the
/// last one left, clipping included, and every sample is written again.
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
    let apply = ui
        .add_enabled(state.gain != 0.0, egui::Button::new(RESCALE))
        .on_hover_text(
            "scales every sample by this gain; Revert goes back to the file as it was saved",
        );
    if apply.clicked() {
        sets.push((GAIN.to_string(), format!("{:.1}", state.gain)));
        state.gain = 0.0;
        return;
    }
    let (said, ink) = match state.read.clipped(state.gain) {
        Ok(0) => ("nothing clips".to_string(), app::caption(ui.visuals())),
        Ok(clipped) => (
            crate::strings::counted(clipped as usize, "sample clips", "samples clip"),
            app::warn(ui.visuals()),
        ),
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

    fn gained(bytes: &[u8], db: &str) -> Result<(Vec<u8>, usize), String> {
        apply(bytes, &[(GAIN.to_string(), db.to_string())])
    }

    #[test]
    fn six_db_doubles_a_quiet_sample() {
        let (out, clipped) = gained(&mono(&[1000, -1000, 0]), "6").unwrap();
        let samples = wav::read_pcm16(&out).unwrap().samples;
        // 10^(6/20) = 1.995, so 1000 becomes 1995.
        assert_eq!(samples, [1995, -1995, 0]);
        assert_eq!(clipped, 0);
    }

    #[test]
    fn a_gain_counts_the_samples_it_clips() {
        let bytes = mono(&[20_000, -20_000, 100, i16::MIN]);
        let (out, clipped) = gained(&bytes, "6.0").unwrap();
        assert_eq!(
            wav::read_pcm16(&out).unwrap().samples,
            [i16::MAX, i16::MIN, 200, i16::MIN]
        );
        assert_eq!(clipped, 3);
    }

    #[test]
    fn a_gain_changes_only_the_samples_of_the_data_chunk() {
        let mut bytes = mono(&[1000, 2000]);
        let list: &[u8] = b"LIST\x04\x00\x00\x00abcd";
        bytes.splice(36..36, list.iter().copied());
        let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) + list.len() as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        let data = wav::pcm16_layout(&bytes).unwrap().data;

        let (out, _) = gained(&bytes, "-6").unwrap();
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
        bytes[34] = 24;
        assert!(gained(&bytes, "6").unwrap_err().contains("24-bit"));
    }

    #[test]
    fn the_clipping_a_gain_would_cause_is_counted_before_it_is_applied() {
        let bytes = mono(&[20_000, -20_000, 100, 16_000]);
        let mut log = crate::log::Log::default();
        let mut workspace = crate::workspace::Workspace::new(crate::testing::context());
        let origin = crate::workspace::Origin::Fresh;
        let id = workspace.ingest("hit.wav".into(), origin, bytes.clone(), &mut log);
        let read = Read::of(workspace.get(id).unwrap());
        for db in ["6.0", "0.0", "-12.0", "1.5"] {
            let (_, applied) = gained(&bytes, db).unwrap();
            assert_eq!(
                read.clipped(db.parse().unwrap()),
                Ok(applied as u64),
                "{db}"
            );
        }
        assert_eq!(read.clipped(6.0), Ok(2));
    }

    #[test]
    fn the_header_states_the_rate_channels_and_length() {
        assert_eq!(facts(44_100, 1, 44_100), "44100 Hz · mono · 1.000 s");
        assert_eq!(facts(48_000, 2, 72_000), "48000 Hz · stereo · 1.500 s");
        assert_eq!(facts(44_100, 6, 0), "44100 Hz · 6 channels · 0.000 s");
    }

    #[test]
    fn only_a_riff_wave_container_is_a_wav_by_its_bytes() {
        assert!(is_wav(&mono(&[0; 8])));
        assert!(!is_wav(b"RIFF"));
        assert!(!is_wav(b"not a wav at all"));
        assert!(!is_wav(&[]));
    }
}
