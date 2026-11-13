//! Finding the hits in a take: where each starts, how loud it is, its pitch and its
//! key-up.
//!
//! Everything runs on the take as recorded, after a 15 Hz high-pass, in 10 ms frames
//! of its level. A take is mono here; a stereo one is detected on its channels' mean.

use crate::take::{fft, filter};

/// Seconds per envelope frame.
pub const FRAME: f64 = 0.010;

/// An onset is a rise of this many decibels over the lowest frame of the
/// [`LOOKBACK`] before it.
pub const RISE_DB: f64 = 15.0;
pub const LOOKBACK: f64 = 0.15;

/// Seconds within which two onsets are one hit, the louder.
pub const MIN_GAP: f64 = 1.0;

/// A key-up is a fall of at least this many decibels over [`KEYUP_SPAN`] seconds.
pub const KEYUP_DROP_DB: f64 = 10.0;
pub const KEYUP_SPAN: f64 = 0.2;

/// The key-up starts where the level leaves the note's slow decay by this much.
pub const KEYUP_LEAVE_DB: f64 = 3.0;

/// Frames of `seconds`, truncated.
pub(crate) fn frames(seconds: f64) -> usize {
    (seconds / FRAME) as usize
}

/// Samples of `seconds` at `rate`, truncated.
pub(crate) fn samples(seconds: f64, rate: u32) -> usize {
    (seconds * f64::from(rate)) as usize
}

/// The level of a silent frame, in dB under full scale: [`Envelope::of`] adds 1e-9 to
/// each frame's RMS.
pub const SILENT_DB: f64 = -180.0;

/// The level of a take in [`FRAME`]s: each frame's RMS, in dB under full scale.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    /// Samples per frame.
    pub width: usize,
    pub db: Vec<f64>,
    /// How its noise floor is read.
    pub floor: Floor,
}

/// How an [`Envelope`]'s noise floor is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Floor {
    /// The level 10% of its frames are under: a take of many notes rests between
    /// them for far more than that.
    #[default]
    Take,
    /// The mean level of its quietest [`QUIETEST`] seconds: a one-note file holds its
    /// note for most of its length and may rest only after the key-up.
    OneNote,
}

/// Seconds of a one-note file's quietest stretch its floor is read on.
pub const QUIETEST: f64 = 0.5;

impl Envelope {
    pub fn of(x: &[f64], rate: u32) -> Envelope {
        let width = samples(FRAME, rate).max(1);
        let db = x
            .chunks_exact(width)
            .map(|frame| {
                let power = frame.iter().map(|v| v * v).sum::<f64>() / width as f64;
                20.0 * (power.sqrt() + 1e-9).log10()
            })
            .collect();
        Envelope {
            width,
            db,
            floor: Floor::Take,
        }
    }

    /// The noise floor, read as [`Envelope::floor`] says, or a silent frame's for a take
    /// shorter than one frame.
    pub fn floor(&self) -> f64 {
        if self.db.is_empty() {
            return SILENT_DB;
        }
        match self.floor {
            Floor::Take => percentile(&self.db, 10.0),
            Floor::OneNote => quietest_mean(&self.db, frames(QUIETEST).max(1)),
        }
    }
}

/// The lowest mean power of any `span` frames running of `db`, in dB, or of all of
/// them when there are fewer.
fn quietest_mean(db: &[f64], span: usize) -> f64 {
    let power: Vec<f64> = db.iter().map(|&d| 10f64.powf(d / 10.0)).collect();
    let span = span.min(power.len());
    let lowest = power
        .windows(span)
        .map(|w| w.iter().sum::<f64>() / span as f64)
        .fold(f64::INFINITY, f64::min);
    10.0 * lowest.log10()
}

/// The `p`th percentile of `values`, interpolated between the nearest two.
///
/// # Panics
///
/// If `values` is empty.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = p / 100.0 * (sorted.len() - 1) as f64;
    let below = at.floor() as usize;
    let above = (below + 1).min(sorted.len() - 1);
    sorted[below] + (sorted[above] - sorted[below]) * (at - below as f64)
}

pub(crate) fn median(values: &[f64]) -> f64 {
    percentile(values, 50.0)
}

/// `x` with the rumble and offset under 15 Hz taken out, as the detectors read it.
pub fn highpassed(x: &[f64], rate: u32) -> Vec<f64> {
    filter::run(&filter::highpass(2, 15.0, rate), x)
}

/// The frames where hits start.
///
/// An onset rises [`RISE_DB`] over the [`LOOKBACK`] before it and is still 15 dB over
/// the floor 100 ms later, which a click is not. Of onsets closer than [`MIN_GAP`], the
/// loudest wins. Only the first frame of a rise counts: the frames after it pass too,
/// and a note whose level wavers would drift the onset 30 to 50 ms late.
///
/// A take that starts [`RISE_DB`] over the floor starts with a hit: a sample set's
/// one-note files are cut at the strike, with nothing before it to rise from.
pub fn onsets(env: &Envelope) -> Vec<usize> {
    let db = &env.db;
    let floor = env.floor();
    let (back, gap, hold) = (frames(LOOKBACK), frames(MIN_GAP), frames(0.1));
    let mut found: Vec<(usize, f64)> = Vec::new();
    if db.len() > hold && db[0] - floor >= RISE_DB && db[hold] > floor + 15.0 {
        let loudest = db[..hold].iter().copied().fold(f64::NEG_INFINITY, f64::max);
        found.push((0, loudest));
    }
    let mut rising = false;
    for i in back..db.len().saturating_sub(hold) {
        let was = rising;
        let lowest = db[i - back..i]
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        rising = db[i] - lowest >= RISE_DB;
        if rising && !was && db[i + hold] > floor + 15.0 {
            let loudest = db[i..i + hold]
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            match found.last_mut() {
                Some(last) if i - last.0 < gap => {
                    if loudest > last.1 {
                        *last = (i, loudest);
                    }
                }
                _ => found.push((i, loudest)),
            }
        }
    }
    found.into_iter().map(|(i, _)| i).collect()
}

/// A symmetric Hann window of `len` points.
pub(crate) fn hann(len: usize) -> Vec<f64> {
    if len == 1 {
        return vec![1.0];
    }
    let span = (len - 1) as f64;
    (0..len)
        .map(|n| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / span).cos())
        .collect()
}

/// The magnitude of each bin of `x`, Hann-windowed and padded with zeros to the size of
/// `plan`.
pub(crate) fn spectrum(x: &[f64], plan: &fft::Plan) -> Vec<f64> {
    let windowed: Vec<f64> = x.iter().zip(hann(x.len())).map(|(v, w)| v * w).collect();
    plan.magnitudes(&windowed)
}

/// The fundamental of `x`, as a MIDI note with a fraction, or none in silence.
///
/// The strongest peak from 80 Hz to `top_hz` is some harmonic of it. Each candidate,
/// the peak over 1 to 8, down to 25 Hz, scores the mean of its harmonics' peaks up to
/// `top_hz`, and the highest candidate within 70% of the best score wins: a soft note's
/// noise would otherwise pull it an octave down. Mains hum fooled a harmonic-product
/// estimate on soft treble notes.
pub fn pitch(x: &[f64], rate: u32, top_hz: f64) -> Option<f64> {
    if x.is_empty() {
        return None;
    }
    let n = (1usize << 18).max(x.len().next_power_of_two());
    let spec = spectrum(x, &fft::Plan::new(n));
    let df = f64::from(rate) / n as f64;
    let lo = (80.0 / df) as usize;
    let hi = ((top_hz / df) as usize).min(spec.len());
    let strongest = (lo..hi).fold(lo, |best, i| if spec[i] > spec[best] { i } else { best });
    if spec[strongest] == 0.0 {
        return None;
    }
    let peak = strongest as f64 * df;
    let score = |f: f64| {
        let harmonics: Vec<f64> = (1..=8)
            .map(|h| f64::from(h) * f)
            .filter(|&g| g <= top_hz.max(f))
            .collect();
        harmonics
            .iter()
            .map(|g| {
                let from = (g * 0.985 / df) as usize;
                let to = ((g * 1.015 / df) as usize).min(spec.len() - 1);
                spec[from..=to].iter().copied().fold(0.0, f64::max)
            })
            .sum::<f64>()
            / harmonics.len() as f64
    };
    let candidates: Vec<f64> = (1..=8)
        .map(|k| peak / f64::from(k))
        .filter(|&f| f >= 25.0)
        .collect();
    let scores: Vec<f64> = candidates.iter().map(|&f| score(f)).collect();
    let best = scores.iter().copied().fold(0.0, f64::max);
    let f = candidates
        .iter()
        .zip(&scores)
        .find(|(_, &s)| s >= 0.7 * best)
        .map(|(&f, _)| f)?;
    Some(69.0 + 12.0 * (f / 440.0).log2())
}

/// One hit of a take.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// Its place in the take, counting from 1.
    pub number: usize,
    /// The frame it starts in.
    pub onset: usize,
    /// The frame the next hit starts in, or the take's end.
    pub next: usize,
    /// The loudest frame of its first 100 ms, in dB under full scale.
    pub level_db: f64,
    /// Its fundamental as a MIDI note with a fraction, over 50 to 550 ms in.
    pub pitch: Option<f64>,
}

/// The hits of `x`, a take as [`highpassed`] returns it, with `env` its envelope.
pub fn hits(x: &[f64], rate: u32, env: &Envelope) -> Vec<Hit> {
    let starts = onsets(env);
    starts
        .iter()
        .enumerate()
        .map(|(k, &onset)| {
            let next = starts.get(k + 1).copied().unwrap_or(env.db.len());
            let at = onset * env.width;
            let body = &x
                [(at + samples(0.05, rate)).min(x.len())..(at + samples(0.55, rate)).min(x.len())];
            let early = &env.db[onset..(onset + frames(0.1)).min(env.db.len())];
            Hit {
                number: k + 1,
                onset,
                next,
                level_db: early.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                pitch: pitch(body, rate, 5_000.0),
            }
        })
        .collect()
}

/// The slope and intercept of the least-squares line through `(x, values[x - from])`.
pub(crate) fn line(from: usize, values: &[f64]) -> (f64, f64) {
    let n = values.len() as f64;
    let xs = (0..values.len()).map(|i| (from + i) as f64);
    let mean_x = xs.clone().sum::<f64>() / n;
    let mean_y = values.iter().sum::<f64>() / n;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (x, y) in xs.zip(values) {
        sxy += (x - mean_x) * (y - mean_y);
        sxx += (x - mean_x) * (x - mean_x);
    }
    let slope = sxy / sxx;
    (slope, mean_y - slope * mean_x)
}

/// The frame a key is released in, searched over `start..stop`, or none.
///
/// The steepest [`KEYUP_SPAN`] fall of at least [`KEYUP_DROP_DB`] that starts while
/// the note is 15 dB over the take's floor, then the first window that falls within
/// 3 dB as steeply: on a steady fall every window is about as steep. That window can
/// start after the damper's thump, so the key-up is where the level leaves the note's
/// slow decay before it.
pub fn keyup(env: &Envelope, start: usize, stop: usize) -> Option<usize> {
    let db = &env.db;
    let span = frames(KEYUP_SPAN);
    let floor = env.floor();
    let mut best = KEYUP_DROP_DB;
    let mut at = None;
    for i in start..stop.saturating_sub(span) {
        let drop = db[i] - db[i + span];
        if drop >= best && db[i] > floor + 15.0 {
            (best, at) = (drop, Some(i));
        }
    }
    let mut at = at?;
    if let Some(first) =
        (start..at).find(|&i| db[i] - db[i + span] >= best - 3.0 && db[i] > floor + 15.0)
    {
        at = first;
    }
    Some(keyup_onset(db, start, at, span))
}

/// Where the fall at `at` leaves the slow decay fitted over 0.4 to 0.15 s before it:
/// the first frame up to the window's end off that line by [`KEYUP_LEAVE_DB`] for
/// three frames running, taken back to where a line through the fall crosses it. A
/// thump that goes up keeps the frame it left at.
fn keyup_onset(db: &[f64], start: usize, at: usize, span: usize) -> usize {
    let (Some(a), Some(b)) = (at.checked_sub(frames(0.4)), at.checked_sub(frames(0.15))) else {
        return at;
    };
    if a < start || b - a < 5 {
        return at;
    }
    let (slope, icept) = line(a, &db[a..b]);
    let e = (at + span).min(db.len() - 1);
    let off: Vec<bool> = (b..=e)
        .map(|i| (db[i] - (slope * i as f64 + icept)).abs() > KEYUP_LEAVE_DB)
        .collect();
    let Some(j) = (0..off.len().saturating_sub(2))
        .find(|&i| off[i] && off[i + 1] && off[i + 2])
        .map(|i| b + i)
    else {
        return at;
    };
    let fall = &db[j..(j + 8).min(db.len())];
    if fall.len() < 2 {
        return j;
    }
    let (fslope, ficept) = line(j, fall);
    if fslope >= slope - 0.1 {
        return j;
    }
    let cross = (icept - ficept) / (fslope - slope);
    (j as f64).min((b as f64).max(cross)).round_ties_even() as usize
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A test take: `notes` of (start s, MIDI note, peak dB, key-up s), each a decaying
    /// tone with three partials that the key-up damps within 50 ms, over hiss at
    /// `floor_db`.
    pub(crate) fn take(
        rate: u32,
        seconds: f64,
        notes: &[(f64, f64, f64, f64)],
        floor_db: f64,
    ) -> Vec<f64> {
        let n = samples(seconds, rate);
        let mut x = vec![0.0; n];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let amp = 10f64.powf(floor_db / 20.0) * 3f64.sqrt();
        for v in &mut x {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *v = amp * ((seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0);
        }
        for &(start, note, peak_db, up) in notes {
            let f = 440.0 * 2f64.powf((note - 69.0) / 12.0);
            let peak = 10f64.powf(peak_db / 20.0);
            let from = samples(start, rate);
            for (i, v) in x.iter_mut().enumerate().skip(from) {
                let t = (i - from) as f64 / f64::from(rate);
                let damp = if t < up - start {
                    1.0
                } else {
                    (-(t - (up - start)) * 90.0).exp()
                };
                let env = (-t * 0.7).exp() * damp * (1.0 - (-t * 2_000.0).exp());
                if env < 1e-7 && t > up - start {
                    break;
                }
                let w = 2.0 * std::f64::consts::PI * f * t;
                *v += peak * env * (0.7 * w.sin() + 0.2 * (2.0 * w).sin() + 0.1 * (3.0 * w).sin());
            }
        }
        x
    }

    #[test]
    fn a_one_note_file_s_floor_is_its_quietest_half_second() {
        // a held organ note, its key-off, and 0.6 s of rest: under 10% of the frames
        let db: Vec<f64> = [
            vec![-33.0; 1100],
            (0..20).map(|i| -33.0 - 3.0 * f64::from(i)).collect(),
            vec![-95.0; 60],
        ]
        .concat();
        let take = Envelope {
            width: 441,
            db,
            floor: Floor::Take,
        };
        assert!(take.floor() > -40.0, "{}", take.floor());
        let one = Envelope {
            floor: Floor::OneNote,
            ..take
        };
        assert!((one.floor() + 95.0).abs() < 1e-9, "{}", one.floor());
        let short = Envelope {
            db: vec![-60.0, -70.0],
            ..one
        };
        let mean = 10.0 * ((1e-6 + 1e-7) / 2.0f64).log10();
        assert!(
            (short.floor() - mean).abs() < 1e-9,
            "fewer frames than half a second"
        );
    }

    #[test]
    fn a_percentile_interpolates_between_its_neighbors() {
        assert_eq!(percentile(&[4.0, 1.0, 3.0, 2.0], 50.0), 2.5);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 10.0), 1.4);
    }

    #[test]
    fn hits_are_found_where_they_start_with_their_pitch_and_level() {
        let rate = 35_002;
        let notes = [
            (1.0, 48.0, -12.0, 3.0),
            (4.0, 55.0, -30.0, 5.5),
            (7.0, 67.0, -20.0, 8.2),
        ];
        let x = highpassed(&take(rate, 10.0, &notes, -80.0), rate);
        let env = Envelope::of(&x, rate);
        let found = hits(&x, rate, &env);
        assert_eq!(found.len(), 3, "{found:?}");
        for (hit, &(start, note, peak_db, _)) in found.iter().zip(&notes) {
            assert!((hit.onset as f64 * FRAME - start).abs() <= 0.02, "{hit:?}");
            assert!((hit.pitch.unwrap() - note).abs() < 0.2, "{hit:?}");
            let against = (hit.level_db - found[0].level_db) - (peak_db - notes[0].2);
            assert!(against.abs() < 1.0, "{hit:?}");
        }
        assert_eq!(found[0].next, found[1].onset);
    }

    #[test]
    fn a_click_is_not_a_hit() {
        let rate = 35_002;
        let mut x = take(rate, 4.0, &[(2.0, 60.0, -15.0, 3.5)], -80.0);
        for v in &mut x[samples(0.8, rate)..samples(0.802, rate)] {
            *v += 0.5;
        }
        let x = highpassed(&x, rate);
        let found = hits(&x, rate, &Envelope::of(&x, rate));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!((found[0].onset as f64 * FRAME - 2.0).abs() <= 0.02);
    }

    #[test]
    fn a_key_up_is_found_where_the_damper_starts() {
        let rate = 35_002;
        let x = highpassed(&take(rate, 6.0, &[(0.5, 57.0, -12.0, 3.37)], -80.0), rate);
        let env = Envelope::of(&x, rate);
        let at = keyup(&env, frames(0.8), env.db.len()).unwrap();
        assert!((at as f64 * FRAME - 3.37).abs() <= 0.03, "{at}");
    }
}
