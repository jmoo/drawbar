//! A stroke with the take's noise taken out: hum under and around the note, and the
//! hiss above it as the note dies.
//!
//! A take's noise is measured once, as [`Noise`]; each stroke is then cleaned by its
//! own pitch. The build cleans every stroke of a take this way, and Detect searches for
//! loops on the strokes as the build leaves them.

use crate::take::detect::{self, Envelope};
use crate::take::fft;
use crate::take::hum::{self, Hum};

/// The size of the hiss follow's transform, 59 ms at 35,002 Hz.
pub const HISS_N: usize = 2048;

/// A band holds the note while its power stands this far over the take's hiss, in dB.
pub const HISS_SNR_DB: f64 = 6.0;

/// Seconds of take each stroke's filters run in over before it starts.
pub const RUN_IN: f64 = 2.5;

/// The fundamental of MIDI `note` in equal temperament, in Hz: what a stroke of that
/// root is cleaned by.
pub fn f0(note: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(note) - 69.0) / 12.0)
}

/// The noise of one take.
#[derive(Clone, Debug, PartialEq)]
pub struct Noise {
    pub rate: u32,
    pub hum: Option<Hum>,
    /// The take's hiss, with its hum out: the mean power in each bin of a
    /// [`HISS_N`]-point transform over its quietest stretch. None where it has no
    /// stretch that long.
    pub hiss: Option<Vec<f64>>,
}

impl Noise {
    /// The noise of `mono`, a take at `rate` as [`crate::take::plan::mono`] reads it.
    pub fn of(mono: &[f64], rate: u32) -> Noise {
        let x = detect::highpassed(mono, rate);
        let hum = hum::find(&x, rate, &Envelope::of(&x, rate)).ok();
        let cleaned = match &hum {
            Some(found) => found.removed(&x, rate),
            None => x,
        };
        Noise::measured(rate, hum, &cleaned, &Envelope::of(&cleaned, rate))
    }

    /// The noise of a take at `rate` with `hum`, whose hum `cleaned` is with all of it
    /// out, and `quiet` its envelope.
    pub fn measured(rate: u32, hum: Option<Hum>, cleaned: &[f64], quiet: &Envelope) -> Noise {
        let quietest = hum::quietest(quiet);
        let from = (quietest.start * quiet.width).min(cleaned.len());
        let to = (quietest.end * quiet.width).min(cleaned.len());
        let hiss = (to - from >= HISS_N).then(|| {
            let frames = Stft::new().frames(&cleaned[from..to]);
            let mut mean = vec![0.0; HISS_N / 2 + 1];
            for frame in &frames {
                for (m, &(re, im)) in mean.iter_mut().zip(frame) {
                    *m += (re * re + im * im) / frames.len() as f64;
                }
            }
            mean
        });
        Noise { rate, hum, hiss }
    }

    /// `frames` frames of `take`, `channels` interleaved, from frame `start`, cleaned as
    /// a stroke of fundamental `f0` Hz.
    pub fn stroke(
        &self,
        take: &[f32],
        channels: usize,
        start: usize,
        frames: usize,
        f0: f64,
    ) -> Vec<f32> {
        let from = start.saturating_sub(detect::samples(RUN_IN, self.rate));
        let window = &take[from * channels..(start + frames) * channels];
        let split: Vec<Vec<f64>> = (0..channels)
            .map(|c| {
                window
                    .iter()
                    .skip(c)
                    .step_by(channels)
                    .map(|&v| f64::from(v))
                    .collect()
            })
            .collect();
        let cleaned = self.channels(split, from, start - from, f0);
        (0..frames)
            .flat_map(|i| cleaned.iter().map(move |c| c[i] as f32))
            .collect()
    }

    /// `frames` samples of `mono` from `start`, cleaned as [`Noise::stroke`] cleans
    /// them.
    pub fn stroke_mono(&self, mono: &[f64], start: usize, frames: usize, f0: f64) -> Vec<f64> {
        let from = start.saturating_sub(detect::samples(RUN_IN, self.rate));
        let window = mono[from..start + frames].to_vec();
        self.channels(vec![window], from, start - from, f0)
            .swap_remove(0)
    }

    /// `channels`, each starting at frame `from` of the take, cleaned and with their
    /// first `skip` frames of run-in dropped.
    fn channels(
        &self,
        channels: Vec<Vec<f64>>,
        from: usize,
        skip: usize,
        f0: f64,
    ) -> Vec<Vec<f64>> {
        let at = from as f64 / f64::from(self.rate);
        let highpass = crate::take::filter::highpass(4, 0.7 * f0, self.rate);
        let channels: Vec<Vec<f64>> = channels
            .iter()
            .map(|x| {
                let y = match &self.hum {
                    Some(hum) => hum.under(x, self.rate, at, f0),
                    None => crate::take::filter::run(&highpass, x),
                };
                y[skip..].to_vec()
            })
            .collect();
        match &self.hiss {
            Some(hiss) => follow(channels, hiss, self.rate, f0),
            None => channels,
        }
    }
}

/// A short-time transform of [`HISS_N`] points, a quarter of them apart, under a
/// periodic Hann window. The signal is padded with half a window of zeros at each end,
/// so its first and last samples come back whole.
struct Stft {
    plan: fft::Plan,
    window: Vec<f64>,
}

impl Stft {
    const HOP: usize = HISS_N / 4;

    fn new() -> Stft {
        let window = (0..HISS_N)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / HISS_N as f64).cos())
            .collect();
        Stft {
            plan: fft::Plan::new(HISS_N),
            window,
        }
    }

    fn frames(&self, x: &[f64]) -> Vec<Vec<(f64, f64)>> {
        let mut padded = vec![0.0; HISS_N / 2];
        padded.extend_from_slice(x);
        padded.resize(padded.len() + HISS_N / 2, 0.0);
        let extra = (Self::HOP - (padded.len() - HISS_N) % Self::HOP) % Self::HOP;
        padded.resize(padded.len() + extra, 0.0);
        (0..=(padded.len() - HISS_N) / Self::HOP)
            .map(|t| {
                let span = &padded[t * Self::HOP..t * Self::HOP + HISS_N];
                let windowed: Vec<f64> =
                    span.iter().zip(&self.window).map(|(v, w)| v * w).collect();
                self.plan.forward(&windowed)
            })
            .collect()
    }

    /// The `len` samples `frames` hold, windowed again and overlap-added.
    fn samples(&self, frames: &[Vec<(f64, f64)>], len: usize) -> Vec<f64> {
        let total = (frames.len() - 1) * Self::HOP + HISS_N;
        let (mut y, mut norm) = (vec![0.0; total], vec![0.0; total]);
        for (t, frame) in frames.iter().enumerate() {
            for (i, v) in self.plan.inverse(frame).iter().enumerate() {
                y[t * Self::HOP + i] += v * self.window[i];
                norm[t * Self::HOP + i] += self.window[i] * self.window[i];
            }
        }
        (HISS_N / 2..HISS_N / 2 + len)
            .map(|i| match norm[i] > 1e-10 {
                true => y[i] / norm[i],
                false => 0.0,
            })
            .collect()
    }
}

/// `channels` through a low-pass that follows the note's own top end down as it dies, so
/// the hiss above it goes with it, with `hiss` the take's as [`Noise::hiss`] holds it.
///
/// Per 1/6 octave from 500 Hz, the note is there while its power, on the channels' mean
/// and smoothed over 30 ms, stands [`HISS_SNR_DB`] over the hiss in that band. The
/// cutoff is the top of the highest band where it is, never rising again after the
/// first 50 ms so the strike keeps all its treble, gliding down over 50 ms, never under
/// 1.5 × `f0`, and rolling off at 24 dB an octave. Nothing is removed where the note is
/// above the hiss. Treble boost and Dyno had made that hiss very loud (heard on a
/// Stage 4).
fn follow(channels: Vec<Vec<f64>>, hiss: &[f64], rate: u32, f0: f64) -> Vec<Vec<f64>> {
    let len = channels[0].len();
    let padded: Vec<Vec<f64>> = channels
        .into_iter()
        .map(|mut x| {
            x.resize(len.max(4 * HISS_N), 0.0);
            x
        })
        .collect();
    let stft = Stft::new();
    let mono: Vec<f64> = (0..padded[0].len())
        .map(|i| padded.iter().map(|x| x[i]).sum::<f64>() / padded.len() as f64)
        .collect();
    let spectra = stft.frames(&mono);
    let frames = spectra.len();
    let hz = |bin: usize| bin as f64 * f64::from(rate) / HISS_N as f64;
    let hop = Stft::HOP as f64;
    let smooth = ((0.03 * f64::from(rate) / hop) as usize).max(1);

    let mut top = vec![1.5 * f0; frames];
    let edges: Vec<f64> = (0..60)
        .map(|i| 500.0 * 2f64.powf(f64::from(i) / 6.0))
        .take_while(|&f| f < f64::from(rate) / 2.0)
        .collect();
    for pair in edges.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        let bins: Vec<usize> = (0..=HISS_N / 2)
            .filter(|&b| hz(b) >= lo && hz(b) < hi)
            .collect();
        if bins.is_empty() {
            continue;
        }
        let floor: f64 = bins.iter().map(|&b| hiss[b]).sum();
        let power: Vec<f64> = spectra
            .iter()
            .map(|frame| {
                bins.iter()
                    .map(|&b| frame[b].0 * frame[b].0 + frame[b].1 * frame[b].1)
                    .sum()
            })
            .collect();
        let lead = (smooth - 1) / 2;
        for (t, top) in top.iter_mut().enumerate() {
            let sum: f64 = (0..smooth)
                .filter_map(|j| (t + lead).checked_sub(j))
                .filter_map(|i| power.get(i))
                .sum();
            if 10.0 * (sum / smooth as f64 / floor + 1e-30).log10() > HISS_SNR_DB {
                *top = top.max(hi);
            }
        }
    }
    let settle = ((0.05 * f64::from(rate) / hop) as usize).min(frames);
    let strike = top[..settle].iter().copied().fold(0.0, f64::max);
    let mut lowest = f64::INFINITY;
    for t in top.iter_mut().skip(settle) {
        lowest = lowest.min(*t);
        *t = match settle {
            0 => lowest,
            _ => lowest.min(strike),
        };
    }
    let glide = (hop / (0.05 * f64::from(rate))).min(1.0);
    let mut cutoff: Vec<f64> = top.iter().map(|t| t.ln()).collect();
    for t in settle + 1..frames {
        cutoff[t] = cutoff[t - 1] + (cutoff[t] - cutoff[t - 1]) * glide;
    }

    padded
        .iter()
        .map(|x| {
            let mut spectra = stft.frames(x);
            for (frame, ln_fc) in spectra.iter_mut().zip(&cutoff) {
                let fc = ln_fc.exp();
                for (b, bin) in frame.iter_mut().enumerate() {
                    let gain = 1.0 / (1.0 + (hz(b) / fc).powi(8)).sqrt();
                    *bin = (bin.0 * gain, bin.1 * gain);
                }
            }
            stft.samples(&spectra, len)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// White noise at `db` under full scale, the same each run.
    fn hiss(len: usize, db: f64) -> Vec<f64> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let scale = 10f64.powf(db / 20.0) * 3f64.sqrt();
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state as f64 / u64::MAX as f64 * 2.0 - 1.0) * scale
            })
            .collect()
    }

    fn rms(x: &[f64]) -> f64 {
        (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn the_short_time_transform_gives_back_the_samples() {
        let x = hiss(10_000, -20.0);
        let stft = Stft::new();
        let back = stft.samples(&stft.frames(&x), x.len());
        for (i, (a, b)) in x.iter().zip(&back).enumerate() {
            assert!((a - b).abs() < 1e-9, "sample {i}");
        }
    }

    /// Where the note stands over the hiss nothing changes; once it has died, the hiss
    /// above it goes, and the note under the cutoff stays.
    #[test]
    fn the_hiss_above_a_dying_note_goes_with_it() {
        let rate = 35_002;
        let f0 = 220.0;
        let floor = hiss(rate as usize * 2, -70.0);
        let noise = Noise::measured(rate, None, &floor, &Envelope::of(&floor, rate));
        let len = rate as usize * 3;
        let noisy = hiss(len, -70.0);
        let x: Vec<f64> = (0..len)
            .map(|i| {
                let t = i as f64 / f64::from(rate);
                let partials: f64 = (1..=12)
                    .map(|k| (2.0 * PI * f0 * f64::from(k) * t).sin() * 0.3 / f64::from(k))
                    .sum();
                partials * 10f64.powf(-15.0 * t / 20.0) * f64::from(u8::from(t >= 0.005)) + noisy[i]
            })
            .collect();
        let y = follow(vec![x.clone()], noise.hiss.as_ref().unwrap(), rate, f0).swap_remove(0);
        let highs = |v: &[f64]| {
            let plan = fft::Plan::new(1 << 15);
            let spec = plan.magnitudes(&v[..1 << 15]);
            let from = (8_000.0 / f64::from(rate) * f64::from(1 << 15)) as usize;
            rms(&spec[from..])
        };
        let tail = rate as usize * 2..len;
        assert!(
            20.0 * (highs(&y[tail.clone()]) / highs(&x[tail.clone()])).log10() < -20.0,
            "the hiss over 8 kHz in the tail"
        );
        let strike = 200..2_000;
        let change = 20.0 * (rms(&y[strike.clone()]) / rms(&x[strike])).log10();
        assert!(change.abs() < 0.2, "the strike changed {change} dB");
        let fundamental = |v: &[f64]| {
            v.iter()
                .enumerate()
                .map(|(i, s)| s * (2.0 * PI * f0 * i as f64 / f64::from(rate)).sin())
                .sum::<f64>()
        };
        let kept = fundamental(&y[tail.clone()]) / fundamental(&x[tail]);
        assert!(
            (kept - 1.0).abs() < 0.05,
            "the fundamental in the tail: {kept}"
        );
    }
}
