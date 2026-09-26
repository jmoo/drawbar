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

/// `C(n, k)`, for the small orders a header can express.
pub fn binomial(n: usize, k: usize) -> i64 {
    let mut c = 1i64;
    for i in 0..k {
        c = c * (n - i) as i64 / (i + 1) as i64;
    }
    c
}

/// One sample: `residual` integrated against `history`, which then carries it.
///
/// The terms saturate instead of wrapping, so a stream that runs the recurrence past
/// `i64` yields a clamped sample its caller can report. A wrapped sample would be
/// indistinguishable from signal.
pub fn predict(history: &mut [i64; MAX_ORDER], order: usize, residual: i64) -> i64 {
    let mut value = residual;
    for j in 1..=order {
        let term = binomial(order, j).saturating_mul(history[j - 1]);
        value = if j.is_multiple_of(2) {
            value.saturating_sub(term)
        } else {
            value.saturating_add(term)
        };
    }
    history.copy_within(0..MAX_ORDER - 1, 1);
    history[0] = value;
    value
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
        for order in 0..=MAX_ORDER {
            let samples: Vec<i64> = (0..12).map(|n: i64| n * n * n - 4 * n).collect();
            let residual = |n: usize| -> i64 {
                (0..=order)
                    .map(|j| {
                        let sign = if j.is_multiple_of(2) { 1 } else { -1 };
                        let at = n as isize - j as isize;
                        sign * binomial(order, j) * usize::try_from(at).map_or(0, |at| samples[at])
                    })
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
    fn the_binomials_are_the_rows_the_orders_name() {
        let row = |n| (0..=n).map(|k| binomial(n, k)).collect::<Vec<_>>();
        assert_eq!(row(0), [1]);
        assert_eq!(row(1), [1, 1]);
        assert_eq!(row(2), [1, 2, 1]);
        assert_eq!(row(3), [1, 3, 3, 1]);
        assert_eq!(row(MAX_ORDER), [1, 4, 6, 4, 1]);
    }
}
