//! The WAV document: the whole file's waveform, audition through the app's player, and a
//! gain edit.
//!
//! A WAV reads at any encoding a take arrives in (integer PCM of 8 to 32 bits, or float),
//! and is drawn and played as float. A gain rewrites the samples of the data chunk in the
//! file's own encoding and nothing else, so every other chunk the file holds survives it;
//! rounding to 16 bits happens only when a library is built.

use std::ops::{Range, RangeInclusive};

use eframe::egui;
use nord_format::wav::{self, Audio, Encoding, Layout};

use super::controls::Sets;
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
/// Tests the container only: a compressed WAV still opens as one, and its document says
/// why it does not read.
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

/// The file with each `gain` set applied in turn to every sample of its data chunk, and
/// how many samples were clipped.
pub fn apply(bytes: &[u8], sets: &[(String, String)]) -> Result<(Vec<u8>, usize), String> {
    let layout = wav::layout(bytes).map_err(|e| e.to_string())?;
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
        clipped += gain_in_place(
            &mut out[layout.data.clone()],
            layout.format.encoding,
            factor,
        );
    }
    Ok((out, clipped))
}

/// The identity cell a WAV puts on the header: the rate, channels, encoding and length
/// its container declares, read without copying its samples.
pub fn stated(entity: &LocalEntity) -> Option<Cell> {
    if Kind::of(entity) != Kind::Wav {
        return None;
    }
    let layout = wav::layout(&entity.bytes).ok()?;
    Some(Cell {
        label: "Audio",
        body: Body::Read(facts(&layout)),
        hint: "the rate, channels, sample format and length the file declares",
    })
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

/// What a WAV's document keeps between frames: the gain typed so far, and a read of the
/// bytes it is open on.
pub struct State {
    /// In dB, to one decimal.
    pub gain: f64,
    read: Read,
}

/// One channel's envelope, as [`sample::channel_envelope`] takes it.
type Lane = Vec<(f32, f32)>;

/// The integer samples a gain could clip, so the clipping a gain would cause is counted
/// before it is applied, exactly, without going through every sample again.
///
/// Whether a sample clips grows with its distance from zero, so the samples that clip at
/// a gain are the loudest few of each sign: two binary searches find them. Only the
/// samples that clip at the control's top gain are kept, which for a take is a small
/// share of it.
#[derive(Default)]
struct Loud {
    bits: u16,
    /// Ascending: the negative ones, then the positive ones.
    samples: Vec<i32>,
}

impl Loud {
    fn of(bytes: &[u8], layout: &Layout) -> Loud {
        let Encoding::Int { bits } = layout.format.encoding else {
            return Loud::default();
        };
        let top = factor(*GAINS.end()).expect("the top of the range is in it");
        let mut samples: Vec<i32> = ints(bytes, layout.data.clone(), bits)
            .filter(|sample| scaled(*sample, top, bits).1)
            .collect();
        samples.sort_unstable();
        Loud { bits, samples }
    }

    /// How many samples a gain of `factor` clips.
    fn clipped(&self, factor: f64) -> u64 {
        let clips = |sample: &i32| scaled(*sample, factor, self.bits).1;
        let split = self.samples.partition_point(|sample| *sample < 0);
        let (negative, positive) = self.samples.split_at(split);
        let low = negative.partition_point(clips);
        let high = positive.len() - positive.partition_point(|sample| !clips(sample));
        (low + high) as u64
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
    loud: Loud,
    /// Each channel's envelope, and the column count it was taken at.
    lanes: Option<(usize, Vec<Lane>)>,
}

impl Read {
    fn of(entity: &LocalEntity) -> Read {
        let bytes = &entity.bytes;
        let read = wav::layout(bytes).and_then(|layout| Ok((wav::read(bytes)?, layout)));
        let (source, loud) = match read {
            Ok((audio, layout)) => (Ok(audio), Loud::of(bytes, &layout)),
            Err(e) => (Err(e.to_string()), Loud::default()),
        };
        Read {
            stamp: entity.stamp,
            source,
            loud,
            lanes: None,
        }
    }

    /// Each channel's envelope across `columns`.
    fn lanes(&mut self, columns: usize) -> Option<&[Lane]> {
        let audio = self.source.as_ref().ok()?;
        if self.lanes.as_ref().is_none_or(|(at, _)| *at != columns) {
            let lanes = (0..usize::from(audio.channels))
                .map(|channel| {
                    sample::channel_envelope(&audio.samples, audio.channels, Some(channel), columns)
                })
                .collect();
            self.lanes = Some((columns, lanes));
        }
        self.lanes.as_ref().map(|(_, lanes)| lanes.as_slice())
    }

    /// How many samples a gain of `db` would clip.
    fn clipped(&self, db: f64) -> Result<u64, String> {
        Ok(self.loud.clipped(factor(db)?))
    }
}

impl State {
    pub fn new(entity: &LocalEntity) -> State {
        State {
            gain: 0.0,
            read: Read::of(entity),
        }
    }

    /// Read the bytes again where they are no longer the ones read, keeping the gain typed.
    pub fn follow(&mut self, entity: &LocalEntity) {
        if self.read.stamp != entity.stamp {
            self.read = Read::of(entity);
        }
    }

    /// The audio as float, whatever the file stores it as.
    pub fn audio(&self) -> Option<&Audio> {
        self.read.source.as_ref().ok()
    }
}

/// Draw the document: the waveform with Play and the gain over it where the bytes read,
/// and why not where they do not. `true` once the operator has asked to play the file, or
/// to stop it where it is playing.
pub fn ui(ui: &mut egui::Ui, state: &mut State, playing: bool, sets: &mut Sets) -> bool {
    let mut audition = false;
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
    let apply = ui.add_enabled(state.gain != 0.0, egui::Button::new(APPLY_GAIN));
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

    fn gained(bytes: &[u8], db: &str) -> Result<(Vec<u8>, usize), String> {
        apply(bytes, &[(GAIN.to_string(), db.to_string())])
    }

    /// A read of `bytes`, as the document makes it.
    fn read(bytes: &[u8]) -> Read {
        let mut log = crate::log::Log::default();
        let mut workspace = crate::workspace::Workspace::new(crate::testing::context());
        let origin = crate::workspace::Origin::Fresh;
        let id = workspace.ingest("hit.wav".into(), origin, bytes.to_vec(), &mut log);
        Read::of(workspace.get(id).unwrap())
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
        let data = wav::layout(&bytes).unwrap().data;

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
        bytes[34] = 12;
        assert!(gained(&bytes, "6").unwrap_err().contains("12-bit"));
    }

    #[test]
    fn a_gain_keeps_a_24_bit_file_24_bit_and_clips_at_its_own_range() {
        let top = 8_388_607.0;
        let bytes = mono_as(
            Encoding::Int { bits: 24 },
            &[1000.0, -1000.0, 5_000_000.0, -8_388_608.0, 1.0],
        );
        let (out, clipped) = gained(&bytes, "6").unwrap();
        assert_eq!(out.len(), bytes.len());
        assert_eq!(
            wav::layout(&out).unwrap().format.encoding,
            Encoding::Int { bits: 24 }
        );
        // 10^(6/20) = 1.995: below 16 bits' step the low sample still keeps its own
        // value, 1 becoming 2, where rounding to 16 bits would have made it 0.
        assert_eq!(ints_of(&out), [1995, -1995, top as i32, -8_388_608, 2]);
        assert_eq!(clipped, 2);
    }

    #[test]
    fn a_gain_scales_every_integer_width_in_its_own_encoding() {
        for bits in [8, 16, 24, 32] {
            let encoding = Encoding::Int { bits };
            let bytes = mono_as(encoding, &[-50.0, 0.0, 50.0]);
            let (out, clipped) = gained(&bytes, "6").unwrap();
            assert_eq!(wav::layout(&out).unwrap().format.encoding, encoding);
            assert_eq!(ints_of(&out), [-100, 0, 100], "{bits}-bit");
            assert_eq!(clipped, 0, "{bits}-bit");
        }
    }

    #[test]
    fn a_gain_on_a_float_file_stays_float_and_clips_nothing() {
        for bits in [32, 64] {
            let encoding = Encoding::Float { bits };
            let bytes = mono_as(encoding, &[0.25, -0.75, 0.0]);
            let (out, clipped) = gained(&bytes, "6").unwrap();
            assert_eq!(wav::layout(&out).unwrap().format.encoding, encoding);
            let samples = wav::read(&out).unwrap().samples;
            let factor = 10f32.powf(0.3);
            for (got, was) in samples.iter().zip([0.25f32, -0.75, 0.0]) {
                assert!((got - was * factor).abs() < 1e-6, "{bits}: {got} vs {was}");
            }
            // -0.75 at +6 dB is past full scale: float keeps it.
            assert!(samples[1] < -1.0);
            assert_eq!(clipped, 0);
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
            let read = read(&bytes);
            let audio = read.source.as_ref().unwrap();
            assert_eq!(audio.samples, [0.5, -0.25], "{encoding:?}");
        }
    }

    #[test]
    fn the_clipping_a_gain_would_cause_is_counted_before_it_is_applied() {
        let bytes = mono(&[20_000, -20_000, 100, 16_000]);
        let read = read(&bytes);
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
    fn the_clipping_count_matches_the_gain_at_every_width_and_step() {
        for bits in [8, 16, 24, 32] {
            let full = (1i64 << (bits - 1)) as f64;
            // A ramp over the whole range, both signs, with the edges themselves.
            let samples: Vec<f64> = (0..=200)
                .map(|step| ((step as f64 / 100.0 - 1.0) * full).round().min(full - 1.0))
                .collect();
            let bytes = mono_as(Encoding::Int { bits }, &samples);
            let read = read(&bytes);
            for tenths in -240..=240 {
                let db = f64::from(tenths) / 10.0;
                let (_, applied) = gained(&bytes, &format!("{db:.1}")).unwrap();
                assert_eq!(
                    read.clipped(db),
                    Ok(applied as u64),
                    "{bits}-bit at {db} dB"
                );
            }
        }
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
    fn only_a_riff_wave_container_is_a_wav_by_its_bytes() {
        assert!(is_wav(&mono(&[0; 8])));
        assert!(!is_wav(b"RIFF"));
        assert!(!is_wav(b"not a wav at all"));
        assert!(!is_wav(&[]));
    }
}
