//! The resampling kernel: source samples onto the field lattice.
//!
//! Field `f` samples the source at `t(f) = PITCH_NUM·f / PITCH_DEN`, and the value it
//! stores is `Σ G[k][m]·x[⌊t⌋ − m]` over the [`TAPS`] taps of one phase of a stored
//! table: `k` is the fractional part of `t` truncated to nine bits, so the bank has
//! [`PHASES`] phases, and `G[k][m] = h(m + k/512)`, a Kaiser-windowed sinc on the
//! half-open support `d ∈ [−15, 15)`:
//!
//! ```text
//! h(d) = B·sinc(B·d) · I₀(β·√(1 − (d/15)²)) / I₀(β) · 32767/32768
//! B = 1.0429·17501/22050        β = 8
//! ```
//!
//! The cutoff `B` sits 4.29 % above the field rate's Nyquist. The window is `1/I₀(β)`
//! at its edge, not zero, so `d = −15` (phase 0, `m = −15`) carries a real tap and
//! `d = +15` does not. The `32767/32768` is the depth term of a 16-bit source, not a
//! kernel gain: the instrument quantises in the source's own units, and a 16-bit
//! sample's unit is `1/32768` of a full-scale `32767`. Per-phase DC gain is the ideal
//! kernel's, within `1e-4` of unity, with no per-phase normalisation.
//!
//! The form is exact and so are its constants; the table the instrument reads is known
//! to an interval of roughly `1e-6` per tap, inside which a handful of taps sit a few
//! `1e-8` off this ideal. A field whose sum lands within that of a quantiser step can
//! therefore still store one count off the instrument's. Everything is evaluated in
//! `f64` and truncated once, after the sum.
//!
//! Inferred from specimens; not confirmed on hardware.

use super::codec::{PITCH_DEN, PITCH_NUM};
use std::sync::OnceLock;

/// Phases in the bank: the fractional source position truncated to nine bits.
pub const PHASES: usize = 512;

/// Taps per phase: `m` from `−15` through `14`, the support `[−15, 15)` at every phase.
pub const TAPS: usize = 30;

/// The `m` of tap 0. Tap `j` weights the source sample at `⌊t⌋ − (j − FIRST)`.
const FIRST: i128 = 15;

/// Half-width of the support, in source samples.
const HALF_WIDTH: f64 = 15.0;

/// Cutoff of the sinc as a fraction of the source's Nyquist: `1.0429·17501/22050`.
const B: f64 = 0.827_745_708_5;

/// Kaiser window shape.
const BETA: f64 = 8.0;

/// A 16-bit sample's unit as a fraction of full scale.
const DEPTH_16: f64 = 32767.0 / 32768.0;

/// Modified Bessel function of the first kind, order zero, by its power series.
fn bessel_i0(x: f64) -> f64 {
    let quarter_square = (x / 2.0) * (x / 2.0);
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut k = 1.0;
    loop {
        term *= quarter_square / (k * k);
        if term < sum * f64::EPSILON {
            return sum;
        }
        sum += term;
        k += 1.0;
    }
}

/// `sin(πx)/(πx)`, on the magnitude so that mirrored taps are bit-identical.
fn sinc(x: f64) -> f64 {
    let x = x.abs();
    if x == 0.0 {
        return 1.0;
    }
    let y = std::f64::consts::PI * x;
    y.sin() / y
}

/// The kernel at `d` source samples from the sample a tap weights, zero off-support.
fn h(d: f64) -> f64 {
    if !(-HALF_WIDTH..HALF_WIDTH).contains(&d) {
        return 0.0;
    }
    let u = d.abs() / HALF_WIDTH;
    B * sinc(B * d) * bessel_i0(BETA * (1.0 - u * u).sqrt()) / bessel_i0(BETA) * DEPTH_16
}

/// Lazily build the `[phase][tap]` bank.
pub fn taps() -> &'static [[f64; TAPS]; PHASES] {
    static BANK: OnceLock<Box<[[f64; TAPS]; PHASES]>> = OnceLock::new();
    BANK.get_or_init(|| {
        let mut bank = Box::new([[0.0; TAPS]; PHASES]);
        for (phase, row) in bank.iter_mut().enumerate() {
            let fraction = phase as f64 / PHASES as f64;
            for (j, slot) in row.iter_mut().enumerate() {
                *slot = h(j as f64 - FIRST as f64 + fraction);
            }
        }
        bank
    })
}

/// Return field `f` as `(floor(source position), phase)` without index overflow.
pub fn lattice(field: usize) -> (i128, usize) {
    let t = u128::from(PITCH_NUM) * field as u128;
    let remainder = t % u128::from(PITCH_DEN);
    (
        (t / u128::from(PITCH_DEN)) as i128,
        (remainder * PHASES as u128 / u128::from(PITCH_DEN)) as usize,
    )
}

/// Accumulate field `f` at full precision; samples outside `source` are zero.
pub fn accumulate(source: &[i16], field: usize) -> f64 {
    let (base, phase) = lattice(field);
    let row = &taps()[phase];
    let mut acc = 0.0;
    for (j, &tap) in row.iter().enumerate() {
        let at = base + FIRST - j as i128;
        if at >= 0 && at < source.len() as i128 {
            acc += f64::from(source[at as usize]) * tap;
        }
    }
    acc
}

/// Return a field in source units, truncating toward zero once after the full sum.
pub fn field(source: &[i16], at: usize) -> i64 {
    accumulate(source, at).trunc() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bessel_series_matches_tabulated_values() {
        assert_eq!(bessel_i0(0.0), 1.0);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008_4).abs() < 1e-15);
        assert!((bessel_i0(8.0) - 427.564_115_721_804_74).abs() < 1e-12);
    }

    /// `d = −15` is in the support at the window's edge value; `d = +15` is not.
    #[test]
    fn the_support_is_half_open() {
        let edge = B * sinc(B * 15.0) / bessel_i0(BETA) * DEPTH_16;
        assert_eq!(taps()[0][0], edge);
        assert!((edge - 4.79e-5).abs() < 1e-7, "{edge}");
        assert_eq!(h(15.0), 0.0);
        assert_eq!(h(-15.0 - f64::EPSILON * 16.0), 0.0);
        assert_ne!(h(-15.0), 0.0);
    }

    /// `G[k][m] = G[512−k][−m−1]`: the lattice point `−d` reads the same tap.
    #[test]
    fn the_kernel_is_mirror_symmetric_to_the_bit() {
        let bank = taps();
        for phase in 1..PHASES {
            for j in 0..TAPS {
                assert_eq!(
                    bank[phase][j].to_bits(),
                    bank[PHASES - phase][TAPS - 1 - j].to_bits(),
                    "phase {phase} tap {j}"
                );
            }
        }
    }

    /// The ideal sinc's DC gain ripples with phase; nothing renormalises it.
    #[test]
    fn every_phase_sums_near_the_depth_factor() {
        for (phase, row) in taps().iter().enumerate() {
            let sum: f64 = row.iter().sum();
            assert!((sum - DEPTH_16).abs() < 1.5e-4, "phase {phase}: {sum}");
        }
    }

    /// A constant stores one count under itself: the 16-bit depth factor takes the
    /// sum a hair below the source value and the truncation lands on the next count.
    #[test]
    fn a_constant_resamples_one_count_under_itself() {
        let source = vec![1000i16; 4096];
        for f in 20..3000 {
            assert_eq!(field(&source, f), 999, "field {f}");
        }
        let source = vec![-1000i16; 4096];
        for f in 20..3000 {
            assert_eq!(field(&source, f), -999, "field {f}");
        }
    }

    /// The phase is the fractional position truncated, never rounded, to nine bits.
    #[test]
    fn the_phase_truncates_the_fraction() {
        assert_eq!(lattice(0), (0, 0));
        // t(1) = 22050/17501 = 1 + 4549/17501; 4549·512/17501 = 133.08.
        assert_eq!(lattice(1), (1, 133));
        // t(17501) = 22050 exactly.
        assert_eq!(lattice(17501), (22050, 0));
    }

    /// A single sample lights every field whose lattice point is within the support,
    /// and nothing beyond it.
    #[test]
    fn one_impulse_lights_the_kernels_support() {
        let mut source = vec![0i16; 4096];
        source[2048] = 30_000;
        let lit: Vec<usize> = (0..3000).filter(|&f| field(&source, f) != 0).collect();
        let (near, _) = lattice(lit[0]);
        let (far, _) = lattice(lit[lit.len() - 1]);
        assert!(2048 - near <= 15 && far - 2048 <= 14, "{near}..{far}");
        assert!(2048 - near >= 13 && far - 2048 >= 13, "{near}..{far}");
    }
}
