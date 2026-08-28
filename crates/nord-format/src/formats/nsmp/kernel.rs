//! The resampling kernel: source samples onto the field lattice.
//!
//! Field `f` samples the source at `t(f) = PITCH_NUM·f / PITCH_DEN`, and the value it
//! stores is that point of the source reconstructed by one interpolation kernel. The
//! kernel is a [`PHASES`]-phase polyphase bank — a field's taps depend only on
//! `PITCH_NUM·f mod PITCH_DEN`, since that residue is the fractional part of `t(f)`
//! — and every phase has **unity DC gain**, which is why a field is already a sample
//! in the source's own units and dequantising is a shift and nothing more.
//!
//! ⚠️ **The taps here are an approximation.** Measured against a specimen sweep the
//! bank is a Hamming-windowed sinc to about 4e-4 per tap, which is what [`taps`]
//! evaluates; the instrument's own values are not known to the last bit. Audio
//! encoded through this is right to roughly −50 dBFS of the encoder's, not
//! bit-identical to it. Inferred from specimens; not confirmed on hardware.

use super::codec::{PITCH_DEN, PITCH_NUM};
use std::sync::OnceLock;

/// Fixed-point scale of a tap: the real coefficient is `tap / ONE`.
pub const ONE: i64 = 1 << 23;

/// Phases in the bank, one per residue of `PITCH_NUM·f mod PITCH_DEN`.
pub const PHASES: usize = PITCH_DEN as usize;

/// Taps per phase.
///
/// Tap `j` weighs the source sample `16 − j` places past `floor(t(f))`, so the window
/// reaches 15 samples back and 16 forward. The kernel's own support ends inside that.
pub const TAPS: usize = 32;

/// Where tap 0 sits relative to `floor(t(f))`, in source samples.
const FIRST_TAP: i64 = 16;

/// `g(δ) = A·sinc(δ/Z)·(0.543 + 0.457·cos(πδ/L))` for `|δ| < L`, zero beyond.
const A: f64 = 0.827_50;
const Z: f64 = 1.208_02;
const L: f64 = 11.91;

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
    }
}

fn g(delta: f64) -> f64 {
    if delta.abs() >= L {
        return 0.0;
    }
    A * sinc(delta / Z) * (0.543 + 0.457 * (std::f64::consts::PI * delta / L).cos())
}

/// The whole bank, `[phase][tap]`, built once.
///
/// Each phase is scaled so its taps sum to exactly [`ONE`]: the DC gain is unity by
/// construction rather than to within rounding, so constant material resamples to
/// itself and a step keeps its level.
pub fn taps() -> &'static [[i32; TAPS]; PHASES] {
    static BANK: OnceLock<Box<[[i32; TAPS]; PHASES]>> = OnceLock::new();
    BANK.get_or_init(|| {
        let mut bank = Box::new([[0i32; TAPS]; PHASES]);
        for (phase, row) in bank.iter_mut().enumerate() {
            let fraction = phase as f64 / PHASES as f64;
            let real: Vec<f64> = (0..TAPS)
                .map(|j| g(j as f64 - FIRST_TAP as f64 + fraction))
                .collect();
            let gain = ONE as f64 / real.iter().sum::<f64>();
            for (slot, &value) in row.iter_mut().zip(&real) {
                *slot = (value * gain).round() as i32;
            }
            // Rounding leaves the sum a few counts off unity; the largest tap absorbs
            // the difference, which is below its own rounding error.
            let residue = ONE - row.iter().map(|&t| i64::from(t)).sum::<i64>();
            let peak = (0..TAPS).max_by_key(|&j| row[j].abs()).unwrap_or(0);
            row[peak] += residue as i32;
        }
        bank
    })
}

/// The lattice position of field `f`, as `(source sample, phase)`.
///
/// The sample is `floor(t(f))` and the phase indexes [`taps`].
pub fn lattice(field: usize) -> (i64, usize) {
    let t = PITCH_NUM as u64 * field as u64;
    (
        (t / u64::from(PITCH_DEN)) as i64,
        (t % u64::from(PITCH_DEN)) as usize,
    )
}

/// Field `f` of `source`, before quantisation: `Σ x[k]·g(t(f) − k)` scaled by [`ONE`].
///
/// Samples outside `source` read as zero, so the lattice runs from the first field to
/// however far past the last sample a caller asks — the encoder uses that to let the
/// kernel ring out past the end of the input.
pub fn accumulate(source: &[i16], field: usize) -> i64 {
    let (base, phase) = lattice(field);
    let row = &taps()[phase];
    let mut acc = 0i64;
    for (j, &tap) in row.iter().enumerate() {
        let at = base + FIRST_TAP - j as i64;
        if at >= 0 && (at as usize) < source.len() {
            acc += i64::from(source[at as usize]) * i64::from(tap);
        }
    }
    acc
}

/// Field `f` of `source` in source units, truncated toward zero.
///
/// The quantiser's first stage: the products sum at full precision and the total is
/// truncated once, so a field is not the sum of separately rounded contributions.
pub fn field(source: &[i16], at: usize) -> i64 {
    accumulate(source, at) / ONE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_phase_has_unity_dc_gain() {
        for row in taps().iter() {
            assert_eq!(row.iter().map(|&t| i64::from(t)).sum::<i64>(), ONE);
        }
    }

    /// Unity gain at every phase is what makes a field a sample: a constant input
    /// comes back as that constant, wherever the lattice lands between samples.
    #[test]
    fn a_constant_resamples_to_itself() {
        let source = vec![1000i16; 4096];
        // Past the kernel's support at both ends, so no field sees the edges.
        for f in 20..3000 {
            assert_eq!(field(&source, f), 1000, "field {f}");
        }
    }

    #[test]
    fn the_kernel_is_mirror_symmetric() {
        // Tap j of phase u sits at δ = j − 16 + u/277, so −δ is tap 31−j of phase 277−u.
        let bank = taps();
        for phase in 1..PHASES {
            for j in 0..TAPS {
                let a = bank[phase][j];
                let b = bank[PHASES - phase][TAPS - 1 - j];
                // One tap per phase carries the rounding residue that pins the DC gain.
                assert!((a - b).abs() <= 8, "phase {phase} tap {j}: {a} vs {b}");
            }
        }
    }

    /// The lattice is exact rational: 277 fields advance the source by exactly 349
    /// samples, so the phase sequence closes on itself.
    #[test]
    fn the_lattice_closes_after_a_superperiod() {
        for f in 0..1000 {
            let (a, pa) = lattice(f);
            let (b, pb) = lattice(f + PITCH_DEN as usize);
            assert_eq!(pa, pb);
            assert_eq!(b - a, i64::from(PITCH_NUM));
        }
    }

    /// A single sample reads back as the kernel's own shape, which is where the
    /// support ends: nothing 13 samples away contributes.
    #[test]
    fn one_impulse_lights_only_the_kernels_support() {
        let mut source = vec![0i16; 4096];
        source[2048] = 30_000;
        let lit: Vec<usize> = (0..3000).filter(|&f| field(&source, f) != 0).collect();
        let (first, last) = (lit[0], lit[lit.len() - 1]);
        let (near, _) = lattice(first);
        let (far, _) = lattice(last);
        assert!((2048 - near) <= 13 && (far - 2048) <= 13);
    }
}
