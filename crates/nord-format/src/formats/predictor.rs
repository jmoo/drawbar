//! The finite-difference predictor the sample and piano codecs share.
//!
//! A stream states backward differences of some order, and the decoder integrates them
//! against the samples it has already reconstructed:
//!
//! ```text
//! x[n] = r[n] − Σ_{j=1..order} (−1)^j C(order, j) · x[n−j]
//! ```
//!
//! The history holds the [`MAX_ORDER`] most recent samples of one channel, most recent
//! first; a stereo stream keeps one per channel.

/// Highest backward-difference order a header can ask for.
pub const MAX_ORDER: usize = 4;

/// `(−1)^j C(order, j)`, the coefficients of the `order`th backward difference, by
/// order.
pub(crate) const DIFFERENCE: [&[i64]; MAX_ORDER + 1] = [
    &[1],
    &[1, -1],
    &[1, -2, 1],
    &[1, -3, 3, -1],
    &[1, -4, 6, -4, 1],
];

/// One sample: `residual` integrated against `history`, which then carries it.
///
/// The terms saturate instead of wrapping, so a stream that runs the recurrence past
/// `i64` yields a clamped sample its caller can report. A wrapped sample would be
/// indistinguishable from signal.
pub fn predict(history: &mut [i64; MAX_ORDER], order: usize, residual: i64) -> i64 {
    let mut value = residual;
    for (&c, &past) in DIFFERENCE[order][1..].iter().zip(history.iter()) {
        let term = c.abs().saturating_mul(past);
        value = if c < 0 {
            value.saturating_add(term)
        } else {
            value.saturating_sub(term)
        };
    }
    history.copy_within(0..MAX_ORDER - 1, 1);
    history[0] = value;
    value
}

/// `value` saturated to `i16`, adding one to `clipped` when it had to be clamped.
pub fn saturate_i16(value: i64, clipped: &mut usize) -> i16 {
    let narrow = value.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
    *clipped += usize::from(i64::from(narrow) != value);
    narrow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_zero_states_the_sample_outright() {
        let mut history = [7i64; MAX_ORDER];
        assert_eq!(predict(&mut history, 0, -30), -30);
        assert_eq!(history, [-30, 7, 7, 7]);
    }

    #[test]
    fn each_order_integrates_the_differences_it_names() {
        // Δ^order of a run of samples, coded back into the run it came from.
        for (order, row) in DIFFERENCE.iter().enumerate() {
            let samples: Vec<i64> = (0..12).map(|n: i64| n * n * n - 4 * n).collect();
            let residual = |n: usize| -> i64 {
                row.iter()
                    .enumerate()
                    .map(|(j, &c)| c * n.checked_sub(j).map_or(0, |at| samples[at]))
                    .sum()
            };
            let mut history = [0i64; MAX_ORDER];
            let decoded: Vec<i64> = (0..samples.len())
                .map(|n| predict(&mut history, order, residual(n)))
                .collect();
            assert_eq!(decoded, samples, "order {order}");
        }
    }

    #[test]
    fn a_recurrence_that_runs_off_i64_saturates_rather_than_wrapping() {
        let mut history = [i64::MAX; MAX_ORDER];
        assert_eq!(predict(&mut history, 1, 1), i64::MAX);
    }

    #[test]
    fn each_difference_row_annihilates_lower_powers_and_scales_its_own() {
        // Δ^n of k^m is 0 for m < n and n! for m = n, whatever k.
        for (order, row) in DIFFERENCE.iter().enumerate() {
            assert_eq!(row.len(), order + 1, "order {order}");
            let delta = |m: u32| -> i64 {
                row.iter()
                    .enumerate()
                    .map(|(j, &c)| c * (10 - j as i64).pow(m))
                    .sum()
            };
            for m in 0..order as u32 {
                assert_eq!(delta(m), 0, "order {order}, power {m}");
            }
            let factorial: i64 = (1..=order as i64).product();
            assert_eq!(delta(order as u32), factorial, "order {order}");
        }
    }
}
