//! How an attack sustains while its key is held: a loop, or a fade to silence.
//!
//! [`find`] decides, on the take, as prep places the loop and fade marks. [`even_loop`]
//! and [`fade_out`] shape the stroke's audio from those marks, as the build reads it,
//! so a mark moved in an editor is shaped the same way.

use crate::detect::{self, Envelope, FRAME};
use crate::wav::{Loop, Play};

/// How [`find`] places a loop or a fade, and how the build shapes them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SustainOptions {
    /// About how long a loop is, in seconds. 100 ms loops repeated the hiss audibly and
    /// beat against the note (heard on a Stage 4).
    pub loop_seconds: f64,
    /// How long the crossfade at a loop's end is, in seconds.
    pub crossfade_seconds: f64,
    /// A note that falls within this of the floor before its key-up dies, in dB.
    pub die_margin_db: f64,
    /// How far under its peak a dying note fades, in dB. Vendor dying notes loop on
    /// silence 48 to 63 dB under their peak (inferred from specimens).
    pub tail_depth_db: f64,
    /// How long before it dies a note's decay rate is measured over, in seconds.
    pub tail_fit_seconds: f64,
    /// A loop ends where the note sinks to this over the floor, in dB.
    pub loop_snr_db: f64,
    /// The least time from the peak to a loop's search, in seconds.
    pub settle_seconds: f64,
    /// How far a loop stays clear of the damper at the attack's end, in seconds.
    pub damper_seconds: f64,
}

impl Default for SustainOptions {
    fn default() -> SustainOptions {
        SustainOptions {
            loop_seconds: 0.5,
            crossfade_seconds: 0.03,
            die_margin_db: 20.0,
            tail_depth_db: 70.0,
            tail_fit_seconds: 0.4,
            loop_snr_db: 30.0,
            settle_seconds: 0.05,
            damper_seconds: 0.15,
        }
    }
}

/// How an attack sustains, in frames from its start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sustain {
    /// A loop over `start..start + len`, whose end matched its start by `score`, the
    /// normalized correlation of the audio before each.
    Loop {
        start: usize,
        len: usize,
        score: f64,
    },
    /// The note dies before its key-up: from `fade` it fades to silence at `end`.
    Dies { fade: usize, end: usize },
}

impl Sustain {
    /// The loop as a WAV states it, both ends played, in frames from `at`.
    pub fn sample_loop(&self, at: usize) -> Option<Loop> {
        match *self {
            Sustain::Loop { start, len, .. } => Some(Loop {
                start: (at + start) as u32,
                end: (at + start + len - 1) as u32,
                play: Play::Forward,
            }),
            Sustain::Dies { .. } => None,
        }
    }
}

/// The frame with the most of `db`, the first of several.
fn peak(db: &[f64]) -> usize {
    (0..db.len()).fold(0, |best, i| if db[i] > db[best] { i } else { best })
}

/// Where the fade of a note dying at envelope frame `dies` starts, and in which frame
/// it reaches its depth, if it does: it falls at the rate the note fell over the
/// [`SustainOptions::tail_fit_seconds`] before, at least 1 dB/s, slowing to a third
/// of it.
fn fade_line(db: &[f64], dies: usize, options: &SustainOptions) -> (f64, f64, f64) {
    let top = peak(db);
    let fit = (options.tail_fit_seconds / FRAME) as usize;
    let from = (top + 5).max(dies.saturating_sub(fit));
    let (slope, intercept) = match dies >= from + 5 {
        true => detect::line(from, &db[from..dies]),
        false => (0.0, db[dies.min(db.len() - 1)]),
    };
    let slope = slope.min(-FRAME);
    let start = intercept + slope * dies as f64;
    (start, db[top] - options.tail_depth_db, slope)
}

/// How `attack`, mono at `rate` and playing MIDI `note`, sustains over a take whose
/// floor is `floor_db`.
///
/// A note that falls within [`SustainOptions::die_margin_db`] of the floor before the
/// damper, and would finish its fade before then, dies. Any other is looped, about
/// [`SustainOptions::loop_seconds`] long and a whole number of the note's periods,
/// ending clear of the damper or where it sinks to [`SustainOptions::loop_snr_db`]
/// over the floor, tuned within a period for the best match. A loop placed in a dying
/// note sat in the noise and clicked (heard on a Stage 4).
pub fn find(
    attack: &[f64],
    rate: u32,
    note: u8,
    floor_db: f64,
    options: &SustainOptions,
) -> Sustain {
    let sr = f64::from(rate);
    let period = sr / (440.0 * 2f64.powf((f64::from(note) - 69.0) / 12.0));
    let env = Envelope::of(attack, rate);
    let w = env.width;
    let mut end = attack
        .len()
        .saturating_sub(1 + detect::samples(options.damper_seconds, rate));
    let top = peak(&env.db);
    let last = (end / w).min(env.db.len());
    let first_under = |over: f64| {
        (top..last)
            .find(|&i| env.db[i] < floor_db + over)
            .filter(|_| top < last)
    };
    if let Some(dies) = first_under(options.die_margin_db) {
        let (start, target, slope) = fade_line(&env.db, dies, options);
        let n = (2.0 * (target - start) / (slope * 3f64.ln())).ceil() as usize;
        if dies + n <= last {
            return Sustain::Dies {
                fade: dies * w,
                end: (dies + n.max(1)) * w,
            };
        }
    }
    if let Some(weak) = first_under(options.loop_snr_db) {
        end = weak * w;
    }
    let want = (options.loop_seconds * sr / period)
        .round_ties_even()
        .max(1.0)
        * period;
    let crossfade = detect::samples(options.crossfade_seconds, rate).min(want as usize / 2);
    end = end.max(
        top * w
            + detect::samples(options.settle_seconds, rate)
            + (want + period) as usize
            + crossfade,
    );
    end = end.min(attack.len().saturating_sub(1));
    let width = (2.0 * period).max(0.01 * sr) as usize;
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let reference = &attack[end.saturating_sub(width)..end];
    let (mut best, mut len) = (-2.0, want.round() as usize);
    for n in (want - period) as usize..=(want + period) as usize {
        let Some(from) = end.checked_sub(n + width) else {
            break;
        };
        let other = &attack[from..from + width];
        let score =
            dot(reference, other) / (dot(reference, reference) * dot(other, other) + 1e-12).sqrt();
        if score > best {
            (best, len) = (score, n);
        }
    }
    let len = len.min(end);
    Sustain::Loop {
        start: end - len,
        len,
        score: best,
    }
}

/// The 10 ms envelope of `frames`, `channels` interleaved, on their mean.
fn envelope(frames: &[f32], channels: usize, rate: u32) -> Envelope {
    let mono: Vec<f64> = frames
        .chunks_exact(channels)
        .map(|frame| frame.iter().map(|&v| f64::from(v)).sum::<f64>() / channels as f64)
        .collect();
    Envelope::of(&mono, rate)
}

/// Shapes a held stroke's loop over `frames`, `channels` interleaved at `rate`, so its
/// end meets its start: the decay across the loop is evened out, and its last
/// [`SustainOptions::crossfade_seconds`] are crossfaded, on a raised cosine, into the
/// audio before its start.
pub fn even_loop(
    frames: &mut [f32],
    channels: usize,
    rate: u32,
    sample_loop: &Loop,
    options: &SustainOptions,
) {
    let (start, end) = (sample_loop.start as usize, sample_loop.end as usize + 1);
    let env = envelope(&frames[..end * channels], channels, rate);
    let (a, b) = (start / env.width, (end / env.width).min(env.db.len()));
    let falling = match b >= a + 3 {
        true => -detect::line(a, &env.db[a..b]).0 / FRAME,
        false => 0.0,
    };
    let sr = f64::from(rate);
    for i in start..end {
        let lift = 10f64.powf(falling.max(0.0) * (i - start) as f64 / sr / 20.0) as f32;
        for v in &mut frames[i * channels..(i + 1) * channels] {
            *v *= lift;
        }
    }
    let len = end - start;
    let xf = detect::samples(options.crossfade_seconds, rate)
        .min(len / 2)
        .min(start);
    for k in 0..xf {
        let ramp = match xf {
            1 => 0.0,
            _ => 0.5 - 0.5 * (std::f64::consts::PI * k as f64 / (xf - 1) as f64).cos(),
        } as f32;
        let (to, from) = (end - xf + k, start - xf + k);
        for c in 0..channels {
            let before = frames[from * channels + c];
            let v = &mut frames[to * channels + c];
            *v = *v * (1.0 - ramp) + before * ramp;
        }
    }
}

/// How long the silence a dying stroke loops on is, in seconds.
const SILENT_LOOP: f64 = 0.1;

/// A dying stroke: `frames`, `channels` interleaved at `rate`, fading from frame `fade`
/// to silence at their end, then silence to loop on, with that loop.
///
/// The fade follows the note's own decay rate, slowing to a third of it, down to
/// [`SustainOptions::tail_depth_db`] under its peak. It only ever lowers the audio,
/// taking the noise down with the line. A straight line in dB was heard as a fade that
/// stops (heard on a Stage 4). The last 10 ms fade to zero.
pub fn fade_out(
    frames: &[f32],
    channels: usize,
    rate: u32,
    fade: usize,
    options: &SustainOptions,
) -> (Vec<f32>, Loop) {
    let env = envelope(frames, channels, rate);
    let w = env.width;
    let total = frames.len() / channels;
    let mut out = frames.to_vec();
    let dies = (fade / w).min(env.db.len().saturating_sub(1));
    let n = (total / w).saturating_sub(dies).max(1);
    if !env.db.is_empty() {
        let (start, target, _) = fade_line(&env.db, dies, options);
        let power: Vec<f64> = env.db.iter().map(|d| 10f64.powf(d / 10.0)).collect();
        let heard = |i: usize| {
            let i = i.min(power.len() - 1);
            let sum: f64 = (i.saturating_sub(2)..=(i + 2).min(power.len() - 1))
                .map(|j| power[j])
                .sum();
            10.0 * (sum / 5.0).log10()
        };
        let gains: Vec<f64> = (0..=n)
            .map(|k| {
                let line =
                    start + (target - start) * (2.0 * k as f64 / n as f64).ln_1p() / 3f64.ln();
                (line - heard(dies + k)).min(0.0)
            })
            .collect();
        for i in dies * w..total {
            let at = (i as f64 / w as f64 - 0.5 - dies as f64).clamp(0.0, n as f64);
            let k = (at.floor() as usize).min(n - 1);
            let g = gains[k] + (gains[k + 1] - gains[k]) * (at - k as f64);
            let g = 10f64.powf(g / 20.0) as f32;
            for v in &mut out[i * channels..(i + 1) * channels] {
                *v *= g;
            }
        }
    }
    let tail = detect::samples(0.01, rate).min(total);
    for k in 0..tail {
        let g = match tail {
            1 => 0.0,
            _ => 0.5 + 0.5 * (std::f64::consts::PI * k as f64 / (tail - 1) as f64).cos(),
        } as f32;
        let i = total - tail + k;
        for v in &mut out[i * channels..(i + 1) * channels] {
            *v *= g;
        }
    }
    let silent = detect::samples(SILENT_LOOP, rate);
    out.resize((total + silent + 64) * channels, 0.0);
    let sample_loop = Loop {
        start: (total + 32) as u32,
        end: (total + 32 + silent - 1) as u32,
        play: Play::Forward,
    };
    (out, sample_loop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// A tone at MIDI `note`, `seconds` long, falling `db_per_s` from `peak_db`, over
    /// noise at `noise_db`.
    fn tone(
        rate: u32,
        note: u8,
        seconds: f64,
        peak_db: f64,
        db_per_s: f64,
        noise_db: f64,
    ) -> Vec<f64> {
        let f = 440.0 * 2f64.powf((f64::from(note) - 69.0) / 12.0);
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        (0..(seconds * f64::from(rate)) as usize)
            .map(|i| {
                let t = i as f64 / f64::from(rate);
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let noise = ((seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * 1.7;
                let level =
                    10f64.powf((peak_db - db_per_s * t) / 20.0) * (1.0 - (-400.0 * t).exp());
                level * (TAU * f * t).sin() + 10f64.powf(noise_db / 20.0) * noise
            })
            .collect()
    }

    /// A slowly decaying note loops about half a second, a whole number of its periods,
    /// ending clear of the damper, and matches well.
    #[test]
    fn a_sustaining_note_loops_half_a_second_of_whole_periods() {
        let rate = 35_002;
        let x = tone(rate, 57, 4.0, -12.0, 3.0, -100.0);
        let Sustain::Loop { start, len, score } =
            find(&x, rate, 57, -100.0, &SustainOptions::default())
        else {
            panic!("a sustaining note loops");
        };
        let period = f64::from(rate) / 220.0;
        let cycles = len as f64 / period;
        assert!((cycles - 110.0).abs() < 1.0, "{cycles} periods");
        assert!(start + len <= x.len() - 5_250, "clear of the damper");
        assert!(start + len > x.len() - 5_300, "as late as it may be");
        assert!(score > 0.99, "match {score}");
    }

    /// A note that falls into the floor long before its key-up dies, and its fade
    /// starts where it came within 20 dB of the floor.
    #[test]
    fn a_note_that_dies_before_its_key_up_fades() {
        let rate = 35_002;
        let x = tone(rate, 84, 6.0, -12.0, 40.0, -80.0);
        let found = find(&x, rate, 84, -80.0, &SustainOptions::default());
        let Sustain::Dies { fade, end } = found else {
            panic!("dies: {found:?}");
        };
        let fade_s = fade as f64 / f64::from(rate);
        assert!((fade_s - 1.2).abs() < 0.1, "fades from {fade_s:.2} s");
        assert!(end > fade && end < x.len(), "{end}");
    }

    /// Evened out, the loop's last frames before its end are as loud as its first, and
    /// the crossfade leaves the frame before the end equal to the one before the start.
    #[test]
    fn an_evened_loop_meets_its_start() {
        let rate = 35_002;
        let x: Vec<f32> = tone(rate, 57, 3.0, -12.0, 6.0, -120.0)
            .iter()
            .map(|&v| v as f32)
            .collect();
        let options = SustainOptions::default();
        let Sustain::Loop { start, len, .. } = find(
            &x.iter().map(|&v| f64::from(v)).collect::<Vec<_>>(),
            rate,
            57,
            -120.0,
            &options,
        ) else {
            panic!("loops");
        };
        let sample_loop = Sustain::Loop {
            start,
            len,
            score: 1.0,
        }
        .sample_loop(0)
        .unwrap();
        let mut y = x.clone();
        even_loop(&mut y, 1, rate, &sample_loop, &options);
        let level = |s: &[f32]| {
            10.0 * (s.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / s.len() as f64).log10()
        };
        let w = 3_500;
        let (head, tail) = (
            level(&y[start..start + w]),
            level(&y[start + len - w..start + len]),
        );
        assert!(
            (head - tail).abs() < 0.3,
            "head {head:.2} dB, tail {tail:.2} dB"
        );
        let before = level(&x[start + len - w..start + len]);
        assert!(tail - before > 2.5, "lifted {:.2} dB", tail - before);
        assert_eq!(y[start + len - 1], x[start - 1]);
        assert_eq!(y[..start], x[..start]);
    }

    /// A dying stroke fades to silence at its end, never louder than it was, then
    /// loops on 100 ms of silence.
    #[test]
    fn a_dying_stroke_fades_to_silence_and_loops_on_it() {
        let rate = 35_002;
        let x: Vec<f64> = tone(rate, 84, 6.0, -12.0, 40.0, -80.0);
        let options = SustainOptions::default();
        let Sustain::Dies { fade, end } = find(&x, rate, 84, -80.0, &options) else {
            panic!("dies");
        };
        let frames: Vec<f32> = x[..end].iter().map(|&v| v as f32).collect();
        let (y, sample_loop) = fade_out(&frames, 1, rate, fade, &options);
        assert_eq!(y.len(), end + 3_500 + 64);
        assert_eq!(sample_loop.start as usize, end + 32);
        assert_eq!((sample_loop.end - sample_loop.start + 1) as usize, 3_500);
        assert!(y[end..].iter().all(|&v| v == 0.0));
        assert!(y[end - 1].abs() < 1e-9);
        assert_eq!(y[..fade], frames[..fade]);
        assert!(y
            .iter()
            .zip(&frames)
            .all(|(a, b)| a.abs() <= b.abs() + 1e-9));
        let tail = &y[end - 1_400..end - 350];
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(
            20.0 * f64::from(peak).log10() < -60.0,
            "near its depth: {peak}"
        );
    }
}
