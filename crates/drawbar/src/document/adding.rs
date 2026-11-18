//! A hit, release or loop added where the take marks none: placed where the take's
//! level puts it and named by its pitch and loudness, or, where those are not found,
//! placed at the operator's click with lengths to drag, so an add never fails for want
//! of a hit.

use std::ops::Range;

use nord_format::formats::npno::encode::{self, LayerTag};
use nord_format::formats::npno::Bank;
use nord_format::take::detect::{self, Envelope};
use nord_format::wav::Audio;

use super::take::{Hit, Marked, Part};

/// How far before the place the search for its hit reaches, in seconds: a click may
/// fall late in a long hit, and the hit's onset is found only with quiet before it.
const BEFORE: f64 = 8.0;

/// How far after the place the search for its hit, and its key-up, reaches, in seconds.
const AFTER: f64 = 10.0;

/// How far after its click a hit that starts nowhere near it may start, in seconds: a
/// click on a hit's first moments still finds it.
const LATE: f64 = 0.2;

/// How long an attack is where no key-up is found, in seconds.
const ATTACK: f64 = 3.0;

/// How long a release reaches at most, and where it finds no quiet, in seconds.
const RELEASE_MOST: f64 = 2.0;
const RELEASE: f64 = 0.5;

/// How long a release found by its fall lasts at least, in seconds.
const RELEASE_LEAST: f64 = 0.1;

/// How far over the take's noise a release ends, in dB.
const RELEASE_OVER_DB: f64 = 6.0;

/// The note a hit is named by where no pitch is found and no other hit is near.
const MIDDLE_C: u8 = 60;

/// How far past the loudest or the softest of its note's layers a new layer's value is
/// put, where it is louder or softer than all of them.
const LAYER_BEYOND: f64 = 10.0;

/// A hit to add: its note and layer, the label of its attack, and the frames of its
/// attack and its release, where the take runs on past the attack.
#[derive(Clone, Debug, PartialEq)]
pub struct Added {
    pub hit: Hit,
    pub label: String,
    pub attack: Range<u32>,
    pub release: Option<Range<u32>>,
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

/// The hit at `place` in `audio`, whose file marks `marked`: from the hit's
/// onset to its key-up, with a release after, named by its pitch and by its loudness
/// against its note's other layers. A hit an attack holds the onset of, or starts
/// within, is passed over, so a click in the quiet after a hit adds no second one of
/// it, even where its start was trimmed past the onset. Where no hit is found, it starts
/// at `place`; where no key-up is, it lasts [`ATTACK`]; where no pitch is, it is named
/// by the hit nearest it, or as middle C. Neither it nor its release runs past the
/// start of the next stroke the take marks.
pub fn hit(audio: &Audio, marked: &Marked, place: u32) -> Added {
    let rate = f64::from(audio.rate.max(1));
    let secs = |seconds: f64| (seconds * rate) as usize;
    let total = frames_of(audio).max(1);
    let place = (place as usize).min(total - 1);
    let mut onset = place;
    let mut stop = (place + secs(AFTER)).min(total);
    let mut keyup = None;
    let mut pitch = None;
    if let Some(window) = Window::of(audio, place.saturating_sub(secs(BEFORE))..stop) {
        let onsets = detect::onsets(&window.env);
        // A hit an attack holds the onset of, or starts within, is marked: its start
        // may have been trimmed past the onset.
        let marked_over = |hit: Range<usize>| {
            (marked.regions.iter())
                .filter(|mark| matches!(mark.part, Part::Attack | Part::Resonance))
                .any(|mark| {
                    let start = mark.frames.start as usize;
                    mark.frames.contains(&(hit.start as u32)) || hit.contains(&start)
                })
        };
        let hit_of = |i: usize| {
            let end = onsets.get(i + 1).map_or(window.env.db.len(), |&next| next);
            window.frame(onsets[i])..window.frame(end)
        };
        let starts: Vec<usize> = (0..onsets.len())
            .filter(|&i| !marked_over(hit_of(i)))
            .map(|i| onsets[i])
            .collect();
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
            let next = onsets
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
                let top = (f64::from(audio.rate) / 2.0).min(5_000.0);
                pitch = detect::pitch(&window.x[heard], audio.rate, top);
            }
        }
    }
    // The next stroke the take marks, which the new one must not run over.
    let next = (marked.regions.iter())
        .filter(|mark| matches!(mark.part, Part::Attack | Part::Resonance | Part::Release))
        .map(|mark| mark.frames.start as usize)
        .filter(|&start| start > onset)
        .min()
        .unwrap_or(total);
    let end = keyup
        .unwrap_or(onset + secs(ATTACK))
        .min(stop.max(onset + 1))
        .min(next)
        .clamp(onset + 1, total);
    let release = release(audio, onset as u32, end as u32)
        .map(|release| release.start..release.end.min(next as u32))
        .filter(|release| !release.is_empty());
    let root = pitch
        .map(|midi| midi.round())
        .filter(|midi| (21.0..=108.0).contains(midi))
        .map(|midi| midi as u8)
        .or_else(|| nearest_root(marked, onset as u32))
        .unwrap_or(MIDDLE_C);
    let hit = (
        root,
        layer(audio, marked, root, peak_db(audio, onset, secs(0.1))),
    );
    Added {
        hit,
        label: attack_label(hit),
        attack: onset as u32..end as u32,
        release,
    }
}

/// The release of an attack over `attack_start` to `keyup` in `audio`: from the key-up
/// to where the take falls to [`RELEASE_OVER_DB`] over its noise, at most
/// [`RELEASE_MOST`] on, or [`RELEASE`] long where it never does. None where the take
/// ends at the key-up.
pub fn release(audio: &Audio, attack_start: u32, keyup: u32) -> Option<Range<u32>> {
    let rate = f64::from(audio.rate.max(1));
    let secs = |seconds: f64| (seconds * rate) as usize;
    let total = frames_of(audio);
    let keyup = keyup as usize;
    if keyup >= total {
        return None;
    }
    let most = (keyup + secs(RELEASE_MOST)).min(total);
    let fallback = (keyup + secs(RELEASE)).min(total).max(keyup + 1);
    // From before the attack, so the window's floor is the take's quiet, not the tail's.
    let from = (attack_start as usize).saturating_sub(secs(BEFORE));
    let end = Window::of(audio, from..most)
        .and_then(|window| {
            let floor = window.env.floor();
            let first = window.at(keyup) + 1;
            (first..window.env.db.len())
                .find(|&at| window.env.db[at] <= floor + RELEASE_OVER_DB)
                .map(|at| window.frame(at))
        })
        // A release quiet from its start found no fall to end at.
        .filter(|&end| end >= keyup + secs(RELEASE_LEAST))
        .unwrap_or(fallback)
        .clamp(keyup + 1, total);
    Some(keyup as u32..end as u32)
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

/// The root of the hit whose attack starts nearest `frame`.
fn nearest_root(marked: &Marked, frame: u32) -> Option<u8> {
    marked
        .regions
        .iter()
        .filter(|mark| matches!(mark.part, Part::Attack | Part::Resonance))
        .filter_map(|mark| Some((mark.hit?.0, mark.frames.start.abs_diff(frame))))
        .min_by_key(|(_, off)| *off)
        .map(|(root, _)| root)
}

/// The layer a new hit of `root` whose peak is `db` takes: a value between the values
/// of the note's layers either side of it in loudness, the loudest valued lowest, and
/// one no other hit of the note has; or, where the note names its layers by index,
/// the next index.
fn layer(audio: &Audio, marked: &Marked, root: u8, db: f64) -> LayerTag {
    // A note whose layers are named by index takes the next index: the build refuses a
    // note named both ways.
    let indexes: Vec<u8> = (marked.regions.iter())
        .filter_map(|mark| match mark.hit? {
            (r, LayerTag::Index(index)) if r == root => Some(index),
            _ => None,
        })
        .collect();
    if let Some(&most) = indexes.iter().max() {
        return LayerTag::Index(most.saturating_add(1));
    }
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
    let value = (0..=100)
        .filter(|value: &i32| !taken.contains(&(*value as u8)))
        .min_by_key(|value| ((value - target).abs(), *value))
        .unwrap_or(target) as u8;
    LayerTag::Value(value)
}

/// The label of `hit`'s attack.
pub fn attack_label(hit: Hit) -> String {
    encode::stroke_name(hit.0, Bank::Attack, hit.1)
}

/// The label of `hit`'s release.
pub fn release_label(hit: Hit) -> String {
    encode::stroke_name(hit.0, Bank::Release, hit.1)
}

#[cfg(test)]
mod tests {
    use super::super::take;
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
        let added = hit(&audio, &Marked::default(), RATE * 2);
        assert_eq!(added.hit, (57, LayerTag::Value(0)), "A3, the only layer");
        assert_eq!(added.label, "057-b0-v00");
        let onset = f64::from(added.attack.start) / f64::from(RATE);
        assert!((onset - 1.0).abs() < 0.05, "onset {onset}");
        let keyup = f64::from(added.attack.end) / f64::from(RATE);
        assert!((keyup - 3.0).abs() < 0.1, "key-up {keyup}");
        let release = added.release.expect("a release");
        assert_eq!(release.start, added.attack.end);
        let quiet = f64::from(release.end) / f64::from(RATE);
        assert!(quiet > 3.05 && quiet < 3.5, "release end {quiet}");
    }

    /// A click in the quiet after a marked hit adds no second one of it: the hit at
    /// the click is placed there, not over the marked one's onset.
    #[test]
    fn a_hit_already_marked_is_not_marked_again() {
        let audio = take(0.5);
        let marked = Marked {
            regions: vec![take::mark("057-b0-v00", RATE..RATE * 3)],
            loops: Vec::new(),
        };
        let added = hit(&audio, &marked, RATE * 4);
        assert!(
            added.attack.start >= RATE * 3,
            "the attack starts at {}, inside the marked one",
            added.attack.start
        );
    }

    /// A hit whose attack starts a little after its onset, trimmed there, is marked all
    /// the same: a click after it adds no second one of it.
    #[test]
    fn a_hit_trimmed_past_its_onset_is_not_marked_again() {
        let audio = take(0.5);
        let trimmed = RATE + RATE / 50;
        let marked = Marked {
            regions: vec![take::mark("057-b0-v00", trimmed..RATE * 3)],
            loops: Vec::new(),
        };
        let added = hit(&audio, &marked, RATE * 4);
        assert!(
            added.attack.start >= RATE * 3,
            "the attack starts at {}, over the marked hit",
            added.attack.start
        );
    }

    /// An attack added in a gap stops where the next marked stroke starts, and has no
    /// release where it reaches it.
    #[test]
    fn an_attack_added_in_a_gap_stops_at_the_next_stroke() {
        let audio = Audio {
            rate: RATE,
            channels: 1,
            samples: vec![0.0; RATE as usize * 6],
        };
        let marked = Marked {
            regions: vec![
                take::mark("064-b0-v22", 0..1000),
                take::mark("064-b0-v40", RATE * 2..RATE * 3),
            ],
            loops: Vec::new(),
        };
        let added = hit(&audio, &marked, RATE);
        assert_eq!(added.attack, RATE..RATE * 2);
        assert_eq!(added.release, None);
    }

    /// A note held to the take's end has no release: none is made past the audio.
    #[test]
    fn a_note_held_to_the_take_s_end_has_no_release() {
        let mut audio = take(0.5);
        audio.samples.truncate(RATE as usize * 2);
        let added = hit(&audio, &Marked::default(), RATE + RATE / 2);
        assert_eq!(added.attack.end, RATE * 2, "the attack runs to the end");
        assert_eq!(added.release, None);
        assert_eq!(release(&audio, RATE, RATE * 2), None);
        assert!(release(&audio, RATE, RATE * 2 - 1).is_some());
    }

    /// A take at a rate whose Nyquist is under the pitch search's top still names its
    /// hit: an 880 Hz note (A5) at 8 kHz.
    #[test]
    fn a_low_rate_take_s_hit_is_named() {
        let rate = 8_000u32;
        let samples = (0..rate as usize * 4)
            .map(|i| {
                let t = i as f32 / rate as f32;
                match t {
                    t if t < 1.0 => 0.0,
                    t => 0.5 * (-(t - 1.0)).exp() * (std::f32::consts::TAU * 880.0 * t).sin(),
                }
            })
            .collect();
        let audio = Audio {
            rate,
            channels: 1,
            samples,
        };
        let added = hit(&audio, &Marked::default(), rate * 2);
        assert_eq!(added.hit.0, 81, "A5");
    }

    /// Silence has no hit and no pitch: the hit is still added, at the click, named
    /// by the hit nearest it.
    #[test]
    fn where_nothing_is_found_the_hit_still_goes_at_the_click() {
        let audio = Audio {
            rate: RATE,
            channels: 1,
            samples: vec![0.0; RATE as usize * 6],
        };
        let marked = Marked {
            regions: vec![take::mark("064-b0-v22", 0..1000)],
            loops: Vec::new(),
        };
        let added = hit(&audio, &marked, RATE);
        assert_eq!(added.attack.start, RATE);
        assert_eq!(added.attack.end, RATE * 4, "an attack of 3 s");
        assert_eq!(added.hit.0, 64, "named by its neighbor");
        assert_ne!(
            added.hit.1,
            LayerTag::Value(22),
            "a value the note does not have"
        );
        assert_eq!(added.release.map(|r| r.len()), Some(RATE as usize / 2));
        let alone = hit(&audio, &Marked::default(), RATE);
        assert_eq!(alone.hit.0, MIDDLE_C);
    }

    /// A note whose layers are named by index takes the next index, not a value: the
    /// build refuses a note named both ways.
    #[test]
    fn a_note_named_by_index_takes_the_next_index() {
        let audio = take(0.5);
        let marked = Marked {
            regions: vec![take::mark("057-b0-l00", RATE * 5..RATE * 6)],
            loops: Vec::new(),
        };
        let added = hit(&audio, &marked, RATE * 2);
        assert_eq!(added.hit, (57, LayerTag::Index(1)));
        assert_eq!(added.label, "057-b0-l01");
    }

    /// A layer between two of its note's in loudness takes a value between theirs.
    #[test]
    fn a_new_layer_s_value_sits_between_its_neighbors_in_loudness() {
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
        let added = hit(&audio, &marked, RATE * 2);
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
