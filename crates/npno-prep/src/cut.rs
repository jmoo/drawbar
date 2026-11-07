//! Where each hit's attack and release lie in the take.

use std::ops::Range;

use crate::detect::{self, frames, samples, Envelope, Hit};

/// How [`cut`] places a hit's strokes.
#[derive(Clone, Debug, PartialEq)]
pub struct CutOptions {
    /// The strike is the first sample this far under the hit's early peak, in dB.
    pub strike_db: f64,
    /// Key and action noise before the strike is kept from where it first comes this
    /// close to the peak, in dB, within [`CutOptions::pre_noise_max_ms`]; none to keep
    /// none.
    pub pre_noise_db: Option<f64>,
    pub pre_noise_max_ms: f64,
    /// Kept before the strike, or before the noise kept, in ms.
    pub pre_roll_ms: f64,
    /// The longest attack, in seconds.
    pub max_seconds: f64,
    /// The longest release, in seconds.
    pub release_seconds: f64,
    /// A release starts this long before the key-up, in ms.
    pub release_pre_ms: f64,
    /// A release ends where it sinks to this far over the quiet before its hit, in dB,
    /// plus [`CutOptions::release_fade_ms`].
    pub release_end_db: f64,
    pub release_fade_ms: f64,
    /// A release is kept when its key-off peaks this far over the quiet before its hit,
    /// in dB, or the note is still [`CutOptions::release_min_snr_db`] over the floor at
    /// key-up.
    pub keyoff_min_db: f64,
    pub release_min_snr_db: f64,
}

impl Default for CutOptions {
    fn default() -> Self {
        CutOptions {
            strike_db: -12.0,
            pre_noise_db: Some(35.0),
            pre_noise_max_ms: 15.0,
            pre_roll_ms: 1.0,
            max_seconds: 6.0,
            release_seconds: 1.0,
            release_pre_ms: 5.0,
            release_end_db: 6.0,
            release_fade_ms: 40.0,
            keyoff_min_db: 10.0,
            release_min_snr_db: 25.0,
        }
    }
}

/// The take as [`cut`] reads it.
pub struct Take<'a> {
    pub rate: u32,
    /// The take as [`detect::highpassed`] returns it.
    pub x: &'a [f64],
    /// Its envelope.
    pub env: &'a Envelope,
    /// The envelope of the take with its hum taken out, which levels are measured
    /// against the floor on.
    pub quiet: &'a Envelope,
}

/// One hit's strokes, in samples of the take.
#[derive(Clone, Debug, PartialEq)]
pub struct Cuts {
    pub attack: Range<usize>,
    /// The frame of the key-up, if one was found.
    pub keyup: Option<usize>,
    pub release: Option<Range<usize>>,
    /// Why a key-up found has no release.
    pub no_release: Option<String>,
}

fn loudest(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

/// Where `hit`'s attack and release lie.
///
/// The attack starts at the strike, found on the take before any filtering moves it,
/// less the end of any key noise loud enough to hear, and the pre-roll. It ends at the
/// key-up, or where the take rises out of the quiet after the note has died into it,
/// and is at most [`CutOptions::max_seconds`] long. The release runs from just before
/// the key-up to where it sinks into the quiet before its hit. A key-off that stands
/// clear of neither that quiet nor the floor has no release.
pub fn cut(take: &Take<'_>, hit: &Hit, options: &CutOptions) -> Cuts {
    let (rate, x, env) = (take.rate, take.x, take.env);
    let w = env.width;
    let (a0, stop) = (hit.onset * w, (hit.next * w).min(x.len()));
    let back = samples(0.03, rate);
    let look = &x[a0.saturating_sub(back)..(a0 + samples(0.2, rate)).min(x.len())];
    let peak = look.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let first = look
        .iter()
        .position(|v| v.abs() > 10f64.powf(options.strike_db / 20.0) * peak)
        .unwrap_or(0);
    let mut strike = (a0 + first).saturating_sub(back);
    if let Some(pre_noise_db) = options.pre_noise_db {
        let m = samples(options.pre_noise_max_ms / 1000.0, rate);
        let k = samples(0.005, rate);
        let seg: Vec<f64> = x[strike.saturating_sub(m + k)..strike]
            .iter()
            .map(|v| v * v)
            .collect();
        if seg.len() > k {
            let mut sum: f64 = seg[..k].iter().sum();
            let mut levels = vec![sum];
            for i in k..seg.len() {
                sum += seg[i] - seg[i - k];
                levels.push(sum);
            }
            let levels = &levels[levels.len().saturating_sub(m)..];
            let under = 20.0 * (peak + 1e-30).log10() - pre_noise_db;
            if let Some(loud) = levels
                .iter()
                .position(|s| 10.0 * (s / k as f64 + 1e-30).log10() > under)
            {
                strike -= levels.len() - loud;
            }
        }
    }
    let start = strike.saturating_sub(samples(options.pre_roll_ms / 1000.0, rate));

    let quiet = &take.quiet.db;
    let floor = take.quiet.floor();
    let after = hit.onset + frames(0.3);
    let mut keyup = detect::keyup(env, after, hit.next.saturating_sub(frames(0.05)));
    let mut end = match keyup {
        Some(at) => at * w,
        None => {
            let mut end = stop.saturating_sub(samples(0.05, rate));
            let span = after.min(hit.next)..hit.next.min(quiet.len());
            if let Some(q) = quiet[span.clone()].iter().position(|&db| db < floor + 6.0) {
                let q = span.start + q;
                let mut lowest = f64::INFINITY;
                let rise = quiet[q..span.end].iter().position(|&db| {
                    lowest = lowest.min(db);
                    db > lowest + 6.0
                });
                if let Some(rise) = rise {
                    let at = (q + rise).saturating_sub(2).max(q);
                    keyup = Some(at);
                    end = at * w;
                }
            }
            end
        }
    };
    end = end.min(start + samples(options.max_seconds, rate));

    let mut cuts = Cuts {
        attack: start..end.max(start),
        keyup,
        release: None,
        no_release: None,
    };
    let Some(ku) = keyup.filter(|&ku| ku * w < stop) else {
        return cuts;
    };
    let r0 = (ku * w).saturating_sub(samples(options.release_pre_ms / 1000.0, rate));
    let mut r1 = stop
        .saturating_sub(samples(0.05, rate))
        .min(r0 + samples(options.release_seconds, rate));
    let before = &env.db
        [hit.onset.saturating_sub(frames(1.0))..hit.onset.saturating_sub(frames(0.1)).max(1)];
    let gap = detect::median(before);
    let db = &env.db;
    let opening = &db[ku..(ku + frames(0.2)).min(r1 / w).max(ku + 1).min(db.len())];
    let k0 = ku
        + opening
            .iter()
            .enumerate()
            .fold(0, |best, (i, &v)| if v > opening[best] { i } else { best });
    if let Some(q) = db[k0.min(r1 / w)..r1 / w]
        .iter()
        .position(|&v| v < gap + options.release_end_db)
    {
        r1 = r1.min((k0 + q) * w + samples(options.release_fade_ms / 1000.0, rate));
    }
    let keyoff_over = (loudest(&db[ku..(ku + frames(0.2)).min(db.len())]) - gap) * 10.0;
    let keyoff_over = keyoff_over.round() / 10.0;
    let over = loudest(&quiet[ku.saturating_sub(5)..(ku + 5).min(quiet.len())]) - floor;
    if keyoff_over < options.keyoff_min_db && over < options.release_min_snr_db {
        cuts.no_release = Some(format!(
            "its key-off peaks {keyoff_over:.1} dB over the quiet before the hit"
        ));
    } else {
        cuts.release = Some(r0..r1.max(r0));
    }
    cuts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::tests::take;
    use crate::hum;

    fn cuts_of(rate: u32, notes: &[(f64, f64, f64, f64)], seconds: f64) -> Vec<Cuts> {
        let x = detect::highpassed(&take(rate, seconds, notes, -80.0), rate);
        let env = Envelope::of(&x, rate);
        let quiet = match hum::find(&x, rate, &env) {
            Ok(found) => Envelope::of(&found.removed(&x, rate), rate),
            Err(_) => env.clone(),
        };
        let take = Take {
            rate,
            x: &x,
            env: &env,
            quiet: &quiet,
        };
        detect::hits(&x, rate, &env)
            .iter()
            .map(|hit| cut(&take, hit, &CutOptions::default()))
            .collect()
    }

    #[test]
    fn an_attack_runs_from_its_strike_to_its_key_up_and_its_release_follows() {
        let rate = 35_002;
        let notes = [(1.0, 52.0, -12.0, 3.4), (5.0, 59.0, -18.0, 7.25)];
        let found = cuts_of(rate, &notes, 9.0);
        assert_eq!(found.len(), 2);
        for (cuts, &(start, _, _, up)) in found.iter().zip(&notes) {
            let at = |s: usize| s as f64 / f64::from(rate);
            assert!((at(cuts.attack.start) - start).abs() < 0.003, "{cuts:?}");
            assert!((at(cuts.attack.end) - up).abs() < 0.03, "{cuts:?}");
            let release = cuts.release.clone().expect("a release");
            assert!(
                at(release.start) < up && up - at(release.start) < 0.04,
                "{cuts:?}"
            );
            assert!(at(release.end) - up < 0.5, "{cuts:?}");
        }
    }

    #[test]
    fn an_attack_is_capped_at_its_longest() {
        let rate = 35_002;
        let found = cuts_of(rate, &[(1.0, 45.0, -10.0, 9.5)], 11.0);
        let attack = &found[0].attack;
        assert_eq!(attack.len(), samples(6.0, rate));
    }
}
