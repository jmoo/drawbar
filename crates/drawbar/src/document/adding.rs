//! A sample, key-off or loop added where the take marks none: placed where the take's
//! level puts it and named by its pitch and loudness, or, where those are not found,
//! placed at the operator's click with lengths to drag, so an add never fails for want
//! of a hit.

use std::ops::Range;

use nord_format::formats::npno::encode::LayerTag;
use nord_format::take::detect::{self, Envelope};
use nord_format::wav::Audio;

use super::take::{self, Hit, Marked, Part};

/// How far before the place the search for its hit reaches, in seconds: a click may
/// fall late in a long sample, and the hit's onset is found only with quiet before it.
const BEFORE: f64 = 8.0;

/// How far after the place the search for its hit, and its key-up, reaches, in seconds.
const AFTER: f64 = 10.0;

/// How far after its click a hit that starts nowhere near it may start, in seconds: a
/// click on a hit's first moments still finds it.
const LATE: f64 = 0.2;

/// How long an attack is where no key-up is found, in seconds.
const ATTACK: f64 = 3.0;

/// How long a key-off reaches at most, and where it finds no quiet, in seconds.
const KEY_OFF_MOST: f64 = 2.0;
const KEY_OFF: f64 = 0.5;

/// How long a key-off found by its fall lasts at least, in seconds.
const KEY_OFF_LEAST: f64 = 0.1;

/// How far over the take's noise a key-off ends, in dB.
const KEY_OFF_OVER_DB: f64 = 6.0;

/// The note a sample is named by where no pitch is found and no other sample is near.
const MIDDLE_C: u8 = 60;

/// How far past the loudest or the softest of its note's layers a new layer's value is
/// put, where it is louder or softer than all of them.
const LAYER_BEYOND: f64 = 10.0;

/// A sample to add: the hit it names, the label of its attack, and the frames of its
/// attack and its key-off.
#[derive(Clone, Debug, PartialEq)]
pub struct Added {
    pub hit: Hit,
    pub label: String,
    pub attack: Range<u32>,
    pub release: Range<u32>,
}

/// A stretch of the take, high-passed as [`detect`] reads it, and its envelope.
struct Window {
    /// The take frame it starts at.
    from: usize,
    x: Vec<f64>,
    env: Envelope,
}

impl Window {
    fn of(audio: &Audio, frames: Range<usize>) -> Option<Window> {
        let channels = usize::from(audio.channels.max(1));
        let total = audio.samples.len() / channels;
        let frames = frames.start.min(total)..frames.end.min(total);
        let part = Audio {
            rate: audio.rate,
            channels: audio.channels,
            samples: audio.samples[frames.start * channels..frames.end * channels].to_vec(),
        };
        let x = detect::highpassed(&nord_format::take::plan::mono(&part), audio.rate);
        let env = Envelope::of(&x, audio.rate);
        (!env.db.is_empty()).then_some(Window {
            from: frames.start,
            x,
            env,
        })
    }

    /// The take frame envelope frame `at` starts at.
    fn frame(&self, at: usize) -> usize {
        self.from + at * self.env.width
    }

    /// The envelope frame take frame `frame` falls in.
    fn at(&self, frame: usize) -> usize {
        (frame.saturating_sub(self.from) / self.env.width).min(self.env.db.len())
    }
}

fn frames_of(audio: &Audio) -> usize {
    audio.samples.len() / usize::from(audio.channels.max(1))
}

/// The sample of the hit at `place` in `audio`, whose file marks `marked`: from the hit's
/// onset to its key-up, with a key-off after, named by its pitch and by its loudness
/// against its note's other layers. Where no hit is found, it starts at `place`; where
/// no key-up is, it lasts [`ATTACK`]; where no pitch is, it is named by the sample
/// nearest it, or as middle C.
pub fn sample(audio: &Audio, marked: &Marked, place: u32) -> Added {
    let rate = f64::from(audio.rate.max(1));
    let secs = |seconds: f64| (seconds * rate) as usize;
    let total = frames_of(audio).max(1);
    let place = (place as usize).min(total - 1);
    let mut onset = place;
    let mut stop = (place + secs(AFTER)).min(total);
    let mut keyup = None;
    let mut pitch = None;
    if let Some(window) = Window::of(audio, place.saturating_sub(secs(BEFORE))..stop) {
        let starts = detect::onsets(&window.env);
        // The last hit to start by the click, a moment after it allowed; else the
        // nearest.
        let found = starts
            .iter()
            .rev()
            .find(|&&at| window.frame(at) <= place + secs(LATE))
            .or_else(|| {
                starts
                    .iter()
                    .min_by_key(|&&at| window.frame(at).abs_diff(place))
            })
            .copied();
        if let Some(start) = found {
            let next = starts
                .iter()
                .copied()
                .find(|&at| at > start)
                .unwrap_or(window.env.db.len());
            onset = window.frame(start);
            stop = window.frame(next).min(total);
            keyup = detect::keyup(&window.env, start, next).map(|at| window.frame(at));
            let body = onset - window.from;
            let heard =
                (body + secs(0.05)).min(window.x.len())..(body + secs(0.55)).min(window.x.len());
            if heard.len() >= secs(0.1) {
                pitch = detect::pitch(&window.x[heard], audio.rate, 5_000.0);
            }
        }
    }
    let end = keyup
        .unwrap_or(onset + secs(ATTACK))
        .min(stop.max(onset + 1))
        .clamp(onset + 1, total);
    let root = pitch
        .map(|midi| midi.round())
        .filter(|midi| (21.0..=108.0).contains(midi))
        .map(|midi| midi as u8)
        .or_else(|| nearest_root(marked, onset as u32))
        .unwrap_or(MIDDLE_C);
    let value = layer(audio, marked, root, peak_db(audio, onset, secs(0.1)));
    let hit = (root, LayerTag::Value(value));
    Added {
        hit,
        label: attack_label(hit),
        attack: onset as u32..end as u32,
        release: key_off(audio, onset as u32, end as u32),
    }
}

/// The key-off of an attack over `attack_start` to `keyup` in `audio`: from the key-up
/// to where the take falls to [`KEY_OFF_OVER_DB`] over its noise, at most
/// [`KEY_OFF_MOST`] on, or [`KEY_OFF`] long where it never does.
pub fn key_off(audio: &Audio, attack_start: u32, keyup: u32) -> Range<u32> {
    let rate = f64::from(audio.rate.max(1));
    let secs = |seconds: f64| (seconds * rate) as usize;
    let total = frames_of(audio);
    let keyup = (keyup as usize).min(total);
    let most = (keyup + secs(KEY_OFF_MOST)).min(total);
    let fallback = (keyup + secs(KEY_OFF)).min(total).max(keyup + 1);
    // From before the attack, so the window's floor is the take's quiet, not the tail's.
    let from = (attack_start as usize).saturating_sub(secs(BEFORE));
    let end = Window::of(audio, from..most)
        .and_then(|window| {
            let floor = window.env.floor();
            let first = window.at(keyup) + 1;
            (first..window.env.db.len())
                .find(|&at| window.env.db[at] <= floor + KEY_OFF_OVER_DB)
                .map(|at| window.frame(at))
        })
        // A key-off quiet from its start found no fall to end at.
        .filter(|&end| end >= keyup + secs(KEY_OFF_LEAST))
        .unwrap_or(fallback)
        .clamp(keyup + 1, total.max(keyup + 1));
    keyup as u32..end as u32
}

/// A loop to drag, in an attack over `attack`: a quarter-second, or less on a short
/// attack, from three-fifths of the way in, end excluded.
pub fn default_loop(attack: &Range<u32>, rate: u32) -> Range<u32> {
    let len = attack.end - attack.start;
    let start = attack.start + len * 3 / 5;
    let long = (rate / 4).min(len * 3 / 10).max(2);
    start..(start + long).min(attack.end)
}

/// The loudest sample of `audio` over `len` frames from `from`, in dB under full scale.
fn peak_db(audio: &Audio, from: usize, len: usize) -> f64 {
    let channels = usize::from(audio.channels.max(1));
    let total = frames_of(audio);
    let span = from.min(total) * channels..(from + len).min(total) * channels;
    let peak = audio.samples[span]
        .iter()
        .fold(0.0f32, |most, v| most.max(v.abs()));
    20.0 * (f64::from(peak) + 1e-9).log10()
}

/// The root of the sample whose attack starts nearest `frame`.
fn nearest_root(marked: &Marked, frame: u32) -> Option<u8> {
    marked
        .regions
        .iter()
        .filter(|mark| matches!(mark.part, Part::Attack | Part::Resonance))
        .filter_map(|mark| Some((mark.hit?.0, mark.frames.start.abs_diff(frame))))
        .min_by_key(|(_, off)| *off)
        .map(|(root, _)| root)
}

/// The layer value a new sample of `root` whose peak is `db` takes: between the values
/// of the note's layers either side of it in loudness, the loudest valued lowest, and
/// one no other sample of the note has.
fn layer(audio: &Audio, marked: &Marked, root: u8, db: f64) -> u8 {
    let rate = f64::from(audio.rate.max(1));
    let mut known: Vec<(f64, u8)> = marked
        .regions
        .iter()
        .filter(|mark| mark.part == Part::Attack)
        .filter_map(|mark| match mark.hit? {
            (r, LayerTag::Value(value)) if r == root => Some((
                peak_db(audio, mark.frames.start as usize, (0.1 * rate) as usize),
                value,
            )),
            _ => None,
        })
        .collect();
    known.sort_by(|a, b| b.0.total_cmp(&a.0));
    let target = match (known.first(), known.last()) {
        (Some(loudest), _) if db >= loudest.0 => f64::from(loudest.1) - LAYER_BEYOND,
        (_, Some(softest)) if db <= softest.0 => f64::from(softest.1) + LAYER_BEYOND,
        (Some(_), Some(_)) => known
            .windows(2)
            .find(|pair| pair[0].0 >= db && db > pair[1].0)
            .map(|pair| {
                let (louder, softer) = (pair[0], pair[1]);
                let share = (louder.0 - db) / (louder.0 - softer.0).max(1e-9);
                f64::from(louder.1) + share * (f64::from(softer.1) - f64::from(louder.1))
            })
            .unwrap_or(0.0),
        _ => 0.0,
    };
    let target = target.clamp(0.0, 100.0).round() as i32;
    let taken: Vec<u8> = marked
        .regions
        .iter()
        .filter_map(|mark| match mark.hit? {
            (r, LayerTag::Value(value)) if r == root => Some(value),
            _ => None,
        })
        .collect();
    (0..=100)
        .filter(|value: &i32| !taken.contains(&(*value as u8)))
        .min_by_key(|value| ((value - target).abs(), *value))
        .unwrap_or(target) as u8
}

/// The label of `hit`'s attack.
pub fn attack_label(hit: Hit) -> String {
    format!("{:03}-b0-{}", hit.0, take::layer(hit.1))
}

/// The label of `hit`'s key-off.
pub fn release_label(hit: Hit) -> String {
    format!("{:03}-b2-{}", hit.0, take::layer(hit.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// A mono take of a 220 Hz note (A3) struck at 1 s at `level`, held to 3 s, then let
    /// go into near silence, with faint noise throughout.
    fn take(level: f32) -> Audio {
        let total = RATE as usize * 6;
        let mut seed = 1u32;
        let samples = (0..total)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 9) as f32 / (1 << 23) as f32 - 0.5;
                let t = i as f32 / RATE as f32;
                let tone = match t {
                    t if t < 1.0 => 0.0,
                    t if t < 3.0 => level * (-(t - 1.0) * 0.5).exp(),
                    t => level * (-1.0f32).exp() * (-(t - 3.0) * 30.0).exp(),
                };
                tone * (std::f32::consts::TAU * 220.0 * t).sin() + noise * 1e-4
            })
            .collect();
        Audio {
            rate: RATE,
            channels: 1,
            samples,
        }
    }

    #[test]
    fn a_clicked_hit_is_found_and_named_by_its_pitch() {
        let audio = take(0.5);
        let added = sample(&audio, &Marked::default(), RATE * 2);
        assert_eq!(added.hit, (57, LayerTag::Value(0)), "A3, the only layer");
        assert_eq!(added.label, "057-b0-v00");
        let onset = f64::from(added.attack.start) / f64::from(RATE);
        assert!((onset - 1.0).abs() < 0.05, "onset {onset}");
        let keyup = f64::from(added.attack.end) / f64::from(RATE);
        assert!((keyup - 3.0).abs() < 0.1, "key-up {keyup}");
        assert_eq!(added.release.start, added.attack.end);
        let quiet = f64::from(added.release.end) / f64::from(RATE);
        assert!(quiet > 3.05 && quiet < 3.5, "key-off end {quiet}");
    }

    /// Silence has no hit and no pitch: the sample is still added, at the click, named
    /// by the sample nearest it.
    #[test]
    fn where_nothing_is_found_the_sample_still_goes_at_the_click() {
        let audio = Audio {
            rate: RATE,
            channels: 1,
            samples: vec![0.0; RATE as usize * 6],
        };
        let marked = Marked {
            regions: vec![take::mark("064-b0-v22", 0..1000)],
            loops: Vec::new(),
        };
        let added = sample(&audio, &marked, RATE);
        assert_eq!(added.attack.start, RATE);
        assert_eq!(added.attack.end, RATE * 4, "an attack of 3 s");
        assert_eq!(added.hit.0, 64, "named by its neighbour");
        assert_ne!(
            added.hit.1,
            LayerTag::Value(22),
            "a value the note does not have"
        );
        assert_eq!(added.release.len(), RATE as usize / 2);
        let alone = sample(&audio, &Marked::default(), RATE);
        assert_eq!(alone.hit.0, MIDDLE_C);
    }

    /// A layer between two of its note's in loudness takes a value between theirs.
    #[test]
    fn a_new_layer_s_value_sits_between_its_neighbours_in_loudness() {
        let mut audio = take(0.25);
        let loud = take(0.5);
        let soft = take(0.125);
        // The loud layer at 1 s of a second copy, the soft at 1 s of a third: one take.
        audio.samples.extend(&loud.samples);
        audio.samples.extend(&soft.samples);
        let six = RATE * 6;
        let marked = Marked {
            regions: vec![
                take::mark("057-b0-v00", six + RATE..six + RATE * 3),
                take::mark("057-b0-v60", 2 * six + RATE..2 * six + RATE * 3),
            ],
            loops: Vec::new(),
        };
        let added = sample(&audio, &marked, RATE * 2);
        let LayerTag::Value(value) = added.hit.1 else {
            panic!("{:?}", added.hit);
        };
        assert!((20..=40).contains(&value), "v{value}");
    }

    #[test]
    fn a_default_loop_sits_inside_its_attack() {
        let looped = default_loop(&(1000..101_000), RATE);
        assert_eq!(looped, 61_000..61_000 + RATE / 4);
        let short = default_loop(&(0..100), RATE);
        assert!(short.end <= 100 && short.len() >= 2);
    }
}
