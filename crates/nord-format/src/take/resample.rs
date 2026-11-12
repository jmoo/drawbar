//! Rate conversion by a Kaiser-windowed sinc, polyphase over the rates' ratio.
//!
//! The filter is the one `scipy.signal.resample_poly` uses by default (cutoff at the
//! lower Nyquist, ten of the lower rate's periods each side, Kaiser β 5, unit DC gain),
//! so a take resampled here matches the Python prototype's to float precision.

use crate::error::{out_of_bounds, Error};
use crate::wav::Audio;

const BETA: f64 = 5.0;
const HALF_PERIODS: u64 = 10;

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// The zeroth-order modified Bessel function of the first kind, by its power series.
fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > sum * 1e-17 {
        term *= quarter / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

/// The filter as `up` phases of `taps` each. Phase `p` holds the taps for an output
/// whose position on the upsampled grid is `p` past a multiple of `up`; tap `i` of it
/// weights input `first(p) + i`, where `first` is [`Filter::first`].
struct Filter {
    up: u64,
    down: u64,
    half: u64,
    taps: usize,
    phases: Vec<f32>,
}

impl Filter {
    fn new(up: u64, down: u64) -> Self {
        let wide = up.max(down);
        let half = HALF_PERIODS * wide;
        let cutoff = 1.0 / wide as f64;
        let window_scale = bessel_i0(BETA);
        let prototype = |t: i64| {
            let x = cutoff * t as f64;
            let sinc = if t == 0 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            let r = t as f64 / half as f64;
            let window = bessel_i0(BETA * (1.0 - r * r).max(0.0).sqrt()) / window_scale;
            cutoff * sinc * window
        };
        let half_i = half as i64;
        let dc: f64 = (-half_i..=half_i).map(prototype).sum();
        let gain = up as f64 / dc;

        let taps = (2 * half / up + 2) as usize;
        let mut phases = vec![0.0f32; up as usize * taps];
        for p in 0..up {
            let first = Self::offset(p, up, half);
            for i in 0..taps {
                let t = p as i64 - (first + i as i64) * up as i64;
                if t.abs() <= half_i {
                    phases[p as usize * taps + i] = (prototype(t) * gain) as f32;
                }
            }
        }
        Filter {
            up,
            down,
            half,
            taps,
            phases,
        }
    }

    /// The first input a phase-`p` output reads, relative to the input it falls past.
    fn offset(p: u64, up: u64, half: u64) -> i64 {
        (p as i64 - half as i64).div_euclid(up as i64)
            + i64::from((p as i64 - half as i64).rem_euclid(up as i64) != 0)
    }

    /// Output `k` of one channel of `n` frames, `stride` samples apart from `from`.
    fn output(&self, k: u64, input: &[f32], from: usize, stride: usize, n: usize) -> f32 {
        let on_grid = k * self.down;
        let (q, p) = (on_grid / self.up, on_grid % self.up);
        let first = q as i64 + Self::offset(p, self.up, self.half);
        let taps = &self.phases[p as usize * self.taps..][..self.taps];
        let mut sum = 0.0f64;
        for (i, &h) in taps.iter().enumerate() {
            let j = first + i as i64;
            if (0..n as i64).contains(&j) {
                sum += f64::from(h) * f64::from(input[from + j as usize * stride]);
            }
        }
        sum as f32
    }
}

/// `audio` at `rate`, each channel filtered on its own. The output has
/// ceil(frames × rate / audio.rate) frames, and output frame k sits at input time
/// k × audio.rate / rate; the input is taken as silent beyond its ends.
pub fn resample(audio: &Audio, rate: u32) -> Result<Audio, Error> {
    if rate == 0 || audio.rate == 0 {
        return out_of_bounds(format!("{} Hz to {rate} Hz", audio.rate), "positive rates");
    }
    let channels = usize::from(audio.channels);
    if channels == 0 || !audio.samples.len().is_multiple_of(channels) {
        return out_of_bounds(
            format!("{} samples of {} channels", audio.samples.len(), channels),
            "whole frames of at least one channel",
        );
    }
    if rate == audio.rate {
        return Ok(audio.clone());
    }
    let g = gcd(u64::from(rate), u64::from(audio.rate));
    let (up, down) = (u64::from(rate) / g, u64::from(audio.rate) / g);
    let filter = Filter::new(up, down);
    let n = audio.frames();
    let frames = (n as u64 * up).div_ceil(down) as usize;
    let mut samples = Vec::with_capacity(frames * channels);
    for k in 0..frames as u64 {
        for c in 0..channels {
            samples.push(filter.output(k, &audio.samples, c, channels, n));
        }
    }
    Ok(Audio {
        rate,
        channels: audio.channels,
        samples,
    })
}

/// The ratio nearest `x` whose denominator is at most `most`, as numerator and
/// denominator: the continued fraction's last convergent within it, or the
/// semiconvergent after it where that is nearer.
fn nearest_ratio(x: f64, most: u64) -> (u64, u64) {
    let (mut p0, mut q0, mut p1, mut q1) = (0u64, 1u64, 1u64, 0u64);
    let mut rest = x;
    loop {
        let a = rest.floor();
        let q2 = q0 + a as u64 * q1;
        if q2 > most {
            break;
        }
        (p0, q0, p1, q1) = (p1, q1, p0 + a as u64 * p1, q2);
        let fraction = rest - a;
        if fraction < 1e-12 {
            return (p1, q1);
        }
        rest = 1.0 / fraction;
    }
    let k = (most - q0) / q1;
    let (pb, qb) = (p0 + k * p1, q0 + k * q1);
    let off = |p: u64, q: u64| (p as f64 / q as f64 - x).abs();
    match off(p1, q1) <= off(pb, qb) {
        true => (p1, q1),
        false => (pb, qb),
    }
}

/// `audio` pitched by `semitones`, at its own rate: resampled by the ratio of
/// denominator at most 2000 nearest 2^(-semitones/12), so it plays that much higher in
/// as many fewer frames. The prototype pitched its extra roots so.
pub fn pitched(audio: &Audio, semitones: i32) -> Result<Audio, Error> {
    let (up, down) = nearest_ratio(2f64.powf(-f64::from(semitones) / 12.0), 2000);
    let shifted = resample(
        &Audio {
            rate: down as u32,
            ..audio.clone()
        },
        up as u32,
    )?;
    Ok(Audio {
        rate: audio.rate,
        ..shifted
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ratio_is_the_nearest_with_its_denominator() {
        assert_eq!(nearest_ratio(0.75, 2000), (3, 4));
        let (p, q) = nearest_ratio(2f64.powf(-11.0 / 12.0), 2000);
        assert!(q <= 2000);
        assert!(
            (p as f64 / q as f64 - 2f64.powf(-11.0 / 12.0)).abs() < 2e-6,
            "{p}/{q}"
        );
    }

    #[test]
    fn a_tone_pitched_up_an_octave_less_one_plays_that_much_higher() {
        let audio = tone(35_002, 440.0, 35_002, 1);
        let up = pitched(&audio, 11).unwrap();
        assert_eq!(up.rate, 35_002);
        let ratio = 2f64.powf(11.0 / 12.0);
        assert!(
            (up.frames() as f64 - 35_002.0 / ratio).abs() < 2.0,
            "{}",
            up.frames()
        );
        let crossings = up.samples[1_000..up.samples.len() - 1_000]
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count() as f64;
        let seconds = (up.samples.len() - 2_000) as f64 / 35_002.0;
        assert!(
            (crossings / seconds - 440.0 * ratio).abs() < 3.0,
            "{}",
            crossings / seconds
        );
    }

    fn tone(rate: u32, hz: f64, frames: usize, channels: u16) -> Audio {
        let samples = (0..frames)
            .flat_map(|i| {
                let x = (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin() as f32;
                (0..channels).map(move |c| if c == 0 { x * 0.5 } else { -x * 0.25 })
            })
            .collect();
        Audio {
            rate,
            channels,
            samples,
        }
    }

    /// The largest error against the same tone made at the new rate, away from the ends
    /// where the input stops.
    fn worst_error(got: &Audio, want: &Audio, margin: usize) -> f32 {
        let c = usize::from(got.channels);
        got.samples[margin * c..(got.frames() - margin) * c]
            .iter()
            .zip(&want.samples[margin * c..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_tone_comes_out_as_the_same_tone_at_the_new_rate() {
        for (from, to, hz) in [
            (44_100, 35_002, 1000.0),
            (48_000, 35_002, 5_000.0),
            (22_050, 35_002, 440.0),
        ] {
            let got = resample(&tone(from, hz, 4410, 2), to).unwrap();
            let want = tone(to, hz, got.frames(), 2);
            assert_eq!(got.channels, 2);
            let err = worst_error(&got, &want, 200);
            assert!(
                err < 1e-3,
                "{from} -> {to} Hz at {hz} Hz: worst error {err}"
            );
        }
    }

    #[test]
    fn the_output_spans_the_input_at_the_new_rate() {
        let got = resample(&tone(44_100, 100.0, 44_101, 1), 35_002).unwrap();
        assert_eq!((got.rate, got.frames()), (35_002, 35_003)); // ceil(44101 × 17501 / 22050)
    }

    #[test]
    fn tones_above_the_new_nyquist_are_filtered_out() {
        let got = resample(&tone(44_100, 21_000.0, 8820, 1), 35_002).unwrap();
        let peak = got.samples[500..got.frames() - 500]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(
            peak < 0.5 * 10f32.powf(-45.0 / 20.0),
            "21 kHz survived at {peak}, against 0.5 in"
        );
    }

    #[test]
    fn an_impulse_lands_where_its_time_falls_at_the_new_rate() {
        let mut samples = vec![0.0; 2205];
        samples[1050] = 1.0; // 1050 / 44100 s = 833.37 frames at 35002 Hz
        let got = resample(
            &Audio {
                rate: 44_100,
                channels: 1,
                samples,
            },
            35_002,
        )
        .unwrap();
        let peak = (0..got.frames())
            .max_by(|&a, &b| got.samples[a].total_cmp(&got.samples[b]))
            .unwrap();
        assert_eq!(peak, 833);
    }

    #[test]
    fn the_same_rate_is_returned_unchanged_and_bad_rates_are_refused() {
        let a = tone(35_002, 440.0, 100, 2);
        assert_eq!(resample(&a, 35_002).unwrap(), a);
        assert!(resample(&a, 0).is_err());
        let ragged = Audio {
            samples: vec![0.0; 3],
            ..a
        };
        assert!(resample(&ragged, 44_100).is_err());
    }
}
