//! The spectrum of real samples, and back, for power-of-two sizes.
//!
//! An iterative radix-2 transform of half the size, unpacked into the real input's
//! spectrum: plain arithmetic, so the crate needs no FFT dependency and builds for wasm.

use std::f64::consts::PI;

/// A transform of one size, with its twiddle factors computed once.
pub struct Plan {
    n: usize,
    /// `e^(-2πik/n)` for `k` in `0..n/2`.
    twiddles: Vec<(f64, f64)>,
    /// Each stage's factors in a row: the stage of span `2h` at `h - 1..2h - 1`.
    stages: Vec<(f64, f64)>,
}

impl Plan {
    /// # Panics
    ///
    /// If `n` is not a power of two of at least 4.
    pub fn new(n: usize) -> Plan {
        assert!(
            n >= 4 && n.is_power_of_two(),
            "a power of two of at least 4"
        );
        let twiddles = (0..n / 2)
            .map(|k| {
                let angle = -2.0 * PI * k as f64 / n as f64;
                (angle.cos(), angle.sin())
            })
            .collect::<Vec<_>>();
        let mut stages = Vec::with_capacity(n / 2);
        let mut half = 1;
        while half < n / 2 {
            let stride = n / (2 * half);
            stages.extend((0..half).map(|k| twiddles[k * stride]));
            half *= 2;
        }
        Plan {
            n,
            twiddles,
            stages,
        }
    }

    /// The magnitude of each of the `n / 2 + 1` bins of `x`, padded with zeros to `n`.
    ///
    /// # Panics
    ///
    /// If `x` is longer than `n`.
    pub fn magnitudes(&self, x: &[f64]) -> Vec<f64> {
        self.forward(x)
            .iter()
            .map(|&(re, im)| (re * re + im * im).sqrt())
            .collect()
    }

    /// The `n / 2 + 1` bins of `x`, padded with zeros to `n`, as real and imaginary parts.
    ///
    /// # Panics
    ///
    /// If `x` is longer than `n`.
    pub fn forward(&self, x: &[f64]) -> Vec<(f64, f64)> {
        assert!(x.len() <= self.n, "no longer than the transform");
        let half = self.n / 2;
        let at = |i: usize| x.get(i).copied().unwrap_or(0.0);
        let mut z: Vec<(f64, f64)> = (0..half).map(|k| (at(2 * k), at(2 * k + 1))).collect();
        self.complex(&mut z);
        (0..=half)
            .map(|k| {
                let (zr, zi) = z[k % half];
                let (cr, ci) = z[(half - k) % half];
                let (er, ei) = ((zr + cr) / 2.0, (zi - ci) / 2.0);
                let (or, oi) = ((zi + ci) / 2.0, (cr - zr) / 2.0);
                let (tr, ti) = match k {
                    k if k < half => self.twiddles[k],
                    _ => (-1.0, 0.0),
                };
                (er + tr * or - ti * oi, ei + tr * oi + ti * or)
            })
            .collect()
    }

    /// The `n` real samples whose bins are `bins`, as [`Plan::forward`] gives them.
    ///
    /// # Panics
    ///
    /// If there are not `n / 2 + 1` bins.
    pub fn inverse(&self, bins: &[(f64, f64)]) -> Vec<f64> {
        let half = self.n / 2;
        assert_eq!(bins.len(), half + 1, "n / 2 + 1 bins");
        let mut z: Vec<(f64, f64)> = (0..half)
            .map(|k| {
                let (xr, xi) = bins[k];
                let (cr, ci) = (bins[half - k].0, -bins[half - k].1);
                let (er, ei) = ((xr + cr) / 2.0, (xi + ci) / 2.0);
                let (dr, di) = ((xr - cr) / 2.0, (xi - ci) / 2.0);
                let (tr, ti) = self.twiddles[k];
                let (or, oi) = (dr * tr + di * ti, di * tr - dr * ti);
                // conjugated, so the forward transform runs it backwards
                (er - oi, -(ei + or))
            })
            .collect();
        self.complex(&mut z);
        z.iter()
            .flat_map(|&(re, im)| [re / half as f64, -im / half as f64])
            .collect()
    }

    /// The forward transform of `z`, `n / 2` points, in place.
    fn complex(&self, z: &mut [(f64, f64)]) {
        let m = z.len();
        let bits = m.trailing_zeros();
        for i in 0..m {
            let j = i.reverse_bits() >> (usize::BITS - bits);
            if j > i {
                z.swap(i, j);
            }
        }
        let mut half = 1;
        while half < m {
            let factors = &self.stages[half - 1..2 * half - 1];
            for span in z.chunks_exact_mut(2 * half) {
                let (low, high) = span.split_at_mut(half);
                for ((a, b), &(wr, wi)) in low.iter_mut().zip(high.iter_mut()).zip(factors) {
                    let (tr, ti) = (b.0 * wr - b.1 * wi, b.0 * wi + b.1 * wr);
                    *b = (a.0 - tr, a.1 - ti);
                    *a = (a.0 + tr, a.1 + ti);
                }
            }
            half *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spectrum_matches_a_direct_transform() {
        let x: Vec<f64> = (0..23).map(|i| ((i * 7 % 11) as f64 - 5.0) / 3.0).collect();
        let n = 32;
        let fast = Plan::new(n).magnitudes(&x);
        assert_eq!(fast.len(), n / 2 + 1);
        for (k, &got) in fast.iter().enumerate() {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &v) in x.iter().enumerate() {
                let angle = -2.0 * PI * (k * i) as f64 / n as f64;
                re += v * angle.cos();
                im += v * angle.sin();
            }
            assert!((got - (re * re + im * im).sqrt()).abs() < 1e-9, "bin {k}");
        }
    }

    #[test]
    fn the_inverse_gives_back_the_samples() {
        let n = 64;
        let x: Vec<f64> = (0..n).map(|i| ((i * 13 % 17) as f64 - 8.0) / 5.0).collect();
        let plan = Plan::new(n);
        let back = plan.inverse(&plan.forward(&x));
        for (i, (a, b)) in x.iter().zip(&back).enumerate() {
            assert!((a - b).abs() < 1e-12, "sample {i}: {a} {b}");
        }
    }

    #[test]
    fn a_tone_peaks_in_its_bin() {
        let n = 1 << 12;
        let x: Vec<f64> = (0..n)
            .map(|i| (2.0 * PI * 100.0 * i as f64 / n as f64).sin())
            .collect();
        let spec = Plan::new(n).magnitudes(&x);
        let top = (0..spec.len()).fold(0, |b, i| if spec[i] > spec[b] { i } else { b });
        assert_eq!(top, 100);
        assert!((spec[100] - n as f64 / 2.0).abs() < 1e-6);
    }
}
