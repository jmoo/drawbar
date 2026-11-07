//! Mains hum: finding it in a take, and taking it out.
//!
//! A take's quiet stretches can be mostly hum: on one interface take, 93% of the floor
//! was 50 Hz hum. Hum lines stand out of the hiss, and the mains frequency wanders, so it
//! is tracked through the take.

use crate::detect::{self, Envelope};
use crate::fft;
use crate::filter;

/// Lines are notched up to this frequency, where a take's strong ones lie.
pub const TOP_HZ: f64 = 1_250.0;

/// Seconds of take each frequency estimate covers.
pub const TRACK_SECONDS: f64 = 4.0;

/// The hum of one take.
#[derive(Clone, Debug, PartialEq)]
pub struct Hum {
    /// 50 or 60.
    pub mains: u32,
    /// Each harmonic of the mains that stands out of the hiss, with how far over it, in dB.
    pub harmonics: Vec<(u32, f64)>,
    /// The mains frequency through the take: seconds into it, and Hz.
    pub track: Vec<(f64, f64)>,
}

/// Why a take has no [`Hum`] to take out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoHum {
    /// No stretch of a second near the floor to measure it on.
    NoQuiet,
    /// No harmonic stands 20 dB out of the hiss.
    None,
    /// Hum, but no stretch of the take where its frequency could be read.
    Untracked,
}

impl std::fmt::Display for NoHum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NoHum::NoQuiet => "no quiet stretch of 1 s to measure hum on",
            NoHum::None => "no hum found",
            NoHum::Untracked => "hum found, but its frequency could not be tracked",
        })
    }
}

/// The longest run of frames within 3 dB of the floor, as a range of frames.
pub(crate) fn quietest(env: &Envelope) -> std::ops::Range<usize> {
    let under = env.floor() + 3.0;
    let mut best = 0..0;
    let mut from = None;
    for (i, &db) in env.db.iter().chain([f64::INFINITY].iter()).enumerate() {
        match (db < under, from) {
            (true, None) => from = Some(i),
            (false, Some(start)) => {
                if i - start > best.len() {
                    best = start..i;
                }
                from = None;
            }
            _ => {}
        }
    }
    best
}

/// The bins of a spectrum of `n` points at `rate` whose frequency is in `lo..hi` of `f`
/// away, exclusive at both ends.
fn around(
    f: f64,
    lo: f64,
    hi: f64,
    n: usize,
    rate: u32,
    len: usize,
) -> impl Iterator<Item = usize> {
    let df = f64::from(rate) / n as f64;
    let first = ((f - hi) / df).floor().max(0.0) as usize;
    let last = (((f + hi) / df).ceil() as usize).min(len - 1);
    (first..=last).filter(move |&k| {
        let off = (k as f64 * df - f).abs();
        off > lo && off < hi
    })
}

/// Finds the hum in `x`, a take as [`detect::highpassed`] returns it.
///
/// Its harmonics are measured on the take's quietest stretch: each harmonic of 50 or
/// 60 Hz, up to 4 kHz, whose line stands 6 dB over the hiss beside it, if one stands 20.
/// Its frequency is then read every [`TRACK_SECONDS`] from the odd harmonics, where
/// transformer hum is strongest.
pub fn find(x: &[f64], rate: u32, env: &Envelope) -> Result<Hum, NoHum> {
    let quiet = quietest(env);
    let gap = &x[(quiet.start * env.width).min(x.len())..(quiet.end * env.width).min(x.len())];
    if gap.len() < rate as usize {
        return Err(NoHum::NoQuiet);
    }
    let n = (gap.len() * 4).next_power_of_two();
    let power: Vec<f64> = detect::spectrum(gap, &fft::Plan::new(n))
        .iter()
        .map(|m| m * m)
        .collect();
    let over_hiss = |f: f64| {
        let line = around(f, -1.0, 1.5, n, rate, power.len())
            .map(|k| power[k])
            .fold(0.0, f64::max);
        let side: Vec<f64> = around(f, 5.0, 20.0, n, rate, power.len())
            .map(|k| power[k])
            .collect();
        10.0 * (line / (side.iter().sum::<f64>() / side.len() as f64)).log10()
    };
    let strength = |mains: u32| {
        [1, 2, 3, 5]
            .iter()
            .map(|k| over_hiss(f64::from(mains * k)))
            .sum::<f64>()
    };
    let mains = if strength(60) > strength(50) { 60 } else { 50 };
    let harmonics: Vec<(u32, f64)> = (1..=4_000 / mains)
        .map(|k| (k, over_hiss(f64::from(mains * k))))
        .filter(|&(_, db)| db > 6.0)
        .collect();
    if harmonics.iter().all(|&(_, db)| db < 20.0) {
        return Err(NoHum::None);
    }

    let block = detect::samples(TRACK_SECONDS, rate);
    let nf = (block * 8).next_power_of_two();
    let plan = fft::Plan::new(nf);
    let mut track = Vec::new();
    let mut start = 0;
    while start + block < x.len() {
        let spec = detect::spectrum(&x[start..start + block], &plan);
        let mut found: Vec<f64> = Vec::new();
        for k in [3u32, 5, 7, 9, 13, 17] {
            let f = f64::from(mains * k);
            let k = f64::from(k);
            let near: Vec<usize> = around(
                f,
                -1.0,
                0.1 * k * f64::from(mains) / 50.0,
                nf,
                rate,
                spec.len(),
            )
            .collect();
            let side: Vec<f64> = around(f, 0.3 * k, 0.6 * k, nf, rate, spec.len())
                .map(|b| spec[b])
                .collect();
            let side = side.iter().sum::<f64>() / side.len() as f64;
            let Some(&top) = near
                .iter()
                .reduce(|a, b| if spec[*b] > spec[*a] { b } else { a })
            else {
                continue;
            };
            if spec[top] > 10.0 * side {
                found.push(top as f64 * f64::from(rate) / nf as f64 / k);
            }
        }
        if !found.is_empty() {
            track.push((
                (start as f64 + block as f64 / 2.0) / f64::from(rate),
                detect::median(&found),
            ));
        }
        start += block;
    }
    if track.is_empty() {
        return Err(NoHum::Untracked);
    }
    Ok(Hum {
        mains,
        harmonics,
        track,
    })
}

impl Hum {
    /// The mains frequency `seconds` into the take, held at the ends of the track.
    pub fn mains_at(&self, seconds: f64) -> f64 {
        let first = self.track[0];
        let last = self.track[self.track.len() - 1];
        if seconds <= first.0 {
            return first.1;
        }
        if seconds >= last.0 {
            return last.1;
        }
        let k = self.track.partition_point(|&(t, _)| t <= seconds);
        let ((t0, f0), (t1, f1)) = (self.track[k - 1], self.track[k]);
        f0 + (f1 - f0) * (seconds - t0) / (t1 - t0)
    }

    /// `x` with every hum line up to [`TOP_HZ`] notched, each notch 0.6 Hz plus 0.02 Hz
    /// per harmonic wide. The take is filtered in 2 s blocks, each at the mains frequency
    /// of its time with 2 s of run-in, and the blocks are overlap-added under a Hann
    /// window. Nothing else is touched.
    pub fn removed(&self, x: &[f64], rate: u32) -> Vec<f64> {
        let hop = detect::samples(2.0, rate);
        let window: Vec<f64> = detect::hann(2 * hop + 1)[..2 * hop].to_vec();
        let mut padded = vec![0.0; hop];
        padded.extend_from_slice(x);
        padded.resize(x.len() + 2 * hop, 0.0);
        let mut y = vec![0.0; x.len() + 2 * hop];
        for c in (0..x.len() + hop).step_by(hop) {
            let mains = self.mains_at(c as f64 / f64::from(rate));
            let notches: Vec<filter::Section> = self
                .harmonics
                .iter()
                .map(|&(k, _)| (f64::from(k) * mains, 0.6 + 0.02 * f64::from(k)))
                .filter(|&(hz, _)| hz < TOP_HZ)
                .map(|(hz, width)| filter::notch(hz, hz / width, rate))
                .collect();
            let from = c.saturating_sub(hop);
            let to = (c + 2 * hop).min(padded.len());
            let run = filter::run(&notches, &padded[from..to]);
            let m = (2 * hop).min(y.len() - c).min(to - c);
            for i in 0..m {
                y[c + i] += run[c - from + i] * window[i];
            }
        }
        y[hop..hop + x.len()].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::tests::take;
    use crate::detect::FRAME;

    /// A take with hum on the 3rd and 5th harmonics of `mains`, which wanders 0.05 Hz.
    fn hummed(rate: u32, seconds: f64, mains: f64) -> Vec<f64> {
        let mut x = take(rate, seconds, &[(3.0, 60.0, -10.0, 5.0)], -90.0);
        let mut phase = 0.0;
        for (i, v) in x.iter_mut().enumerate() {
            let t = i as f64 / f64::from(rate);
            phase +=
                2.0 * std::f64::consts::PI * (mains + 0.05 * (t / 3.0).sin()) / f64::from(rate);
            *v += 10f64.powf(-70.0 / 20.0) * (3.0 * phase).sin();
            *v += 10f64.powf(-76.0 / 20.0) * (5.0 * phase).sin();
        }
        detect::highpassed(&x, rate)
    }

    #[test]
    fn hum_is_found_on_its_mains_and_taken_out_of_the_quiet() {
        let rate = 35_002;
        let x = hummed(rate, 12.0, 50.0);
        let env = Envelope::of(&x, rate);
        let hum = find(&x, rate, &env).unwrap();
        assert_eq!(hum.mains, 50);
        let found: Vec<u32> = hum.harmonics.iter().map(|&(k, _)| k).collect();
        assert!(found.contains(&3) && found.contains(&5), "{found:?}");
        assert!(
            hum.track.iter().all(|&(_, f)| (f - 50.0).abs() < 0.1),
            "{:?}",
            hum.track
        );

        let clean = Envelope::of(&hum.removed(&x, rate), rate);
        let quiet = detect::frames(7.0)..detect::frames(11.0);
        let mean = |e: &Envelope| e.db[quiet.clone()].iter().sum::<f64>() / quiet.len() as f64;
        assert!(
            mean(&env) - mean(&clean) > 10.0,
            "{} -> {}",
            mean(&env),
            mean(&clean)
        );
        let note = detect::frames(3.2)..detect::frames(4.5);
        for i in note {
            assert!(
                (env.db[i] - clean.db[i]).abs() < 0.5,
                "the note at {}",
                i as f64 * FRAME
            );
        }
    }

    #[test]
    fn a_take_without_hum_has_none_to_take_out() {
        let rate = 35_002;
        let x = detect::highpassed(&take(rate, 6.0, &[(3.0, 60.0, -10.0, 5.0)], -80.0), rate);
        assert_eq!(find(&x, rate, &Envelope::of(&x, rate)), Err(NoHum::None));
    }
}
