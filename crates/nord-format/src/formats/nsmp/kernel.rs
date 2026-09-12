//! The resampling kernel: source samples onto the field lattice.
//!
//! Field `f` samples the source at `t(f) = PITCH_NUM·f / PITCH_DEN`, and the value it
//! stores is `Σ G[k][m]·x[⌊t⌋ − m]` over the [`TAPS`] taps of one phase of a stored
//! table: `k` is the fractional part of `t` truncated to nine bits, so the bank has
//! [`PHASES`] phases, and `G[k][m] = h(m + k/512)`, a Kaiser-windowed sinc on the
//! half-open support `d ∈ [−15, 15)`:
//!
//! ```text
//! h(d) = B·sinc(B·d) · I₀(β·√(1 − (d/15)²)) / I₀(β)
//! B = 1.0429·17501/22050        β = 8
//! ```
//!
//! The cutoff `B` sits 4.29 % above the field rate's Nyquist. The window is `1/I₀(β)`
//! at its edge, not zero, so `d = −15` (phase 0, `m = −15`) carries a real tap and
//! `d = +15` does not. Per-phase DC gain is the ideal kernel's, within `1e-4` of
//! unity, with no per-phase normalisation.
//!
//! The arithmetic is single precision up to the sum. The table holds `h` rounded to
//! `f32`; a 16-bit sample enters as the `f32` product of its value and `32767/32768`
//! — the source's own unit as a fraction of full scale, a depth term rather than a
//! kernel gain, since the instrument quantises in the source's units; each tap's
//! product is an `f32`; the products are summed in `f64` and truncated toward zero
//! once. A few taps are stored off the closed form and are listed by value in
//! `MEASURED`. Where a tap is known only to within an `f32` rounding, a sum landing
//! within that of a quantiser step can still store one count off the instrument's.
//!
//! The ratio is not baked into the bank: [`Kernel`] runs the same shape at another
//! one, which is what a source at a rate other than
//! [`SOURCE_RATE`](super::codec::SOURCE_RATE) needs. What moves with the ratio is the
//! cutoff — it keeps the narrower of the two Nyquists, so a source faster than the
//! target is band-limited to the target's and nothing folds back. At the measured
//! ratio that is `B` to the bit, and the bank is the measured one.
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

/// Cutoff of the sinc as a fraction of the source's Nyquist on the measured lattice:
/// `1.0429·17501/22050`, the field rate's Nyquist and 4.29 % over.
const B: f64 = 0.827_745_708_5;

/// Kaiser window shape.
const BETA: f64 = 8.0;

/// A 16-bit sample's unit as a fraction of full scale; exact in `f32`.
const DEPTH_16: f32 = 32767.0 / 32768.0;

/// Taps whose stored value is not `h` rounded to `f32`, as `(phase, tap, value)`. The
/// values are measured, not derived; each lies within `2e-7` of the closed form.
const MEASURED: [(usize, usize, f32); 88] = [
    (1, 16, 0.15957576),
    (2, 19, -0.050585914),
    (12, 21, 0.0012514186),
    (15, 14, 0.18686648),
    (19, 22, -0.009900425),
    (20, 15, 0.82630205),
    (20, 21, 0.00010499005),
    (21, 16, 0.1264128),
    (25, 13, -0.14319749),
    (29, 8, -0.014026146),
    (37, 21, -0.002284253),
    (49, 13, -0.15284193),
    (54, 9, 0.010980806),
    (54, 16, 0.07440956),
    (64, 23, 0.009098176),
    (71, 8, -0.017184436),
    (75, 21, -0.0073220395),
    (77, 10, 0.001966087),
    (84, 16, 0.030549219),
    (89, 10, -0.00071834825),
    (103, 13, -0.16755463),
    (105, 10, -0.0043659243),
    (128, 22, 0.000063097374),
    (142, 20, 0.034269594),
    (146, 12, 0.07262834),
    (151, 17, -0.03939612),
    (153, 23, 0.0044261394),
    (174, 17, -0.024487488),
    (184, 15, 0.7108851),
    (184, 17, -0.018098239),
    (185, 14, 0.49301848),
    (187, 5, -0.00045713593),
    (199, 5, -0.0008218217),
    (203, 17, -0.0061836657),
    (205, 17, -0.004949704),
    (213, 23, 0.0011110727),
    (215, 10, -0.029679712),
    (228, 11, 0.013030337),
    (231, 21, -0.020947903),
    (234, 14, 0.5760287),
    (238, 15, 0.63809144),
    (242, 10, -0.03536856),
    (245, 9, 0.03330808),
    (246, 16, -0.13485077),
    (253, 6, -0.0012877032),
    (259, 23, -0.0012877032),
    (266, 13, -0.13485077),
    (267, 20, 0.03330808),
    (270, 19, -0.03536856),
    (274, 14, 0.63809144),
    (278, 15, 0.5760287),
    (281, 8, -0.020947903),
    (284, 18, 0.013030337),
    (297, 19, -0.029679712),
    (299, 6, 0.0011110727),
    (307, 12, -0.004949704),
    (309, 12, -0.0061836657),
    (325, 24, -0.00045713593),
    (327, 15, 0.49301848),
    (328, 12, -0.018098239),
    (328, 14, 0.7108851),
    (338, 12, -0.024487488),
    (359, 6, 0.0044261394),
    (361, 12, -0.03939612),
    (366, 17, 0.07262834),
    (370, 9, 0.034269594),
    (405, 13, -0.00059061556),
    (407, 19, -0.0043659243),
    (409, 16, -0.16755463),
    (423, 19, -0.00071834825),
    (428, 13, 0.030549219),
    (435, 19, 0.001966087),
    (441, 21, -0.017184436),
    (448, 6, 0.009098176),
    (458, 13, 0.07440956),
    (458, 20, 0.010980806),
    (463, 16, -0.15284193),
    (475, 8, -0.002284253),
    (483, 21, -0.014026146),
    (487, 16, -0.14319749),
    (491, 13, 0.1264128),
    (492, 14, 0.82630205),
    (493, 7, -0.009900425),
    (497, 15, 0.18686648),
    (500, 8, 0.0012514186),
    (508, 8, 0.0024095366),
    (510, 10, -0.050585914),
    (511, 13, 0.15957576),
];

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

/// `h` of cutoff `b`: the kernel at `d` source samples from the sample a tap
/// weights, zero off-support.
fn h_at(d: f64, b: f64) -> f64 {
    if !(-HALF_WIDTH..HALF_WIDTH).contains(&d) {
        return 0.0;
    }
    let u = d.abs() / HALF_WIDTH;
    b * sinc(b * d) * bessel_i0(BETA * (1.0 - u * u).sqrt()) / bessel_i0(BETA)
}

/// The cutoff a lattice of `num` source samples per `den` fields is band-limited to,
/// as a fraction of the source's Nyquist: the narrower of the two Nyquists, over by
/// the same 4.29 % the measured bank carries. A faster source is cut at the field
/// rate's Nyquist so that nothing folds back; a slower one keeps its own band.
///
/// Exactly [`B`] at the measured ratio, however the caller spells it.
fn cutoff(num: u32, den: u32) -> f64 {
    let keep = f64::from(num.min(den)) / f64::from(num);
    let measured = f64::from(PITCH_DEN) / f64::from(PITCH_NUM);
    B * (keep / measured)
}

/// `h` of cutoff `b` rounded to `f32` at every lattice point of the `[phase][tap]`
/// bank.
fn closed_form_at(b: f64) -> Box<[[f32; TAPS]; PHASES]> {
    let mut bank = Box::new([[0.0; TAPS]; PHASES]);
    for (phase, row) in bank.iter_mut().enumerate() {
        let fraction = phase as f64 / PHASES as f64;
        for (j, slot) in row.iter_mut().enumerate() {
            *slot = h_at(j as f64 - FIRST as f64 + fraction, b) as f32;
        }
    }
    bank
}

/// [`closed_form_at`] at the measured cutoff.
fn closed_form() -> Box<[[f32; TAPS]; PHASES]> {
    closed_form_at(B)
}

/// Lazily build the `[phase][tap]` bank the instrument stores.
pub fn taps() -> &'static [[f32; TAPS]; PHASES] {
    static BANK: OnceLock<Box<[[f32; TAPS]; PHASES]>> = OnceLock::new();
    BANK.get_or_init(|| {
        let mut bank = closed_form();
        for &(phase, tap, value) in &MEASURED {
            bank[phase][tap] = value;
        }
        bank
    })
}

/// Return field `f` as `(floor(source position), phase)` without index overflow, on a
/// lattice of `num` source samples per `den` fields.
pub fn lattice_at(field: usize, num: u32, den: u32) -> (i128, usize) {
    let t = u128::from(num) * field as u128;
    let remainder = t % u128::from(den);
    (
        (t / u128::from(den)) as i128,
        (remainder * PHASES as u128 / u128::from(den)) as usize,
    )
}

/// [`lattice_at`] on the lattice the tap bank was measured for.
pub fn lattice(field: usize) -> (i128, usize) {
    lattice_at(field, PITCH_NUM, PITCH_DEN)
}

/// Sum field `f`'s tap products in `f64` over `bank`; samples outside `source` are
/// zero.
fn accumulate_over(
    bank: &[[f32; TAPS]; PHASES],
    source: &[i16],
    field: usize,
    num: u32,
    den: u32,
) -> f64 {
    let (base, phase) = lattice_at(field, num, den);
    let row = &bank[phase];
    let mut acc = 0.0f64;
    for (j, &tap) in row.iter().enumerate() {
        let at = base + FIRST - j as i128;
        if at >= 0 && at < source.len() as i128 {
            let sample = f32::from(source[at as usize]) * DEPTH_16;
            acc += f64::from(sample * tap);
        }
    }
    acc
}

/// Sum field `f`'s tap products in `f64` on the lattice the tap bank was measured for;
/// samples outside `source` are zero.
pub fn accumulate(source: &[i16], field: usize) -> f64 {
    accumulate_over(taps(), source, field, PITCH_NUM, PITCH_DEN)
}

/// Return a field in source units on the lattice the tap bank was measured for,
/// truncating toward zero once after the full sum.
pub fn field(source: &[i16], at: usize) -> i64 {
    accumulate(source, at).trunc() as i64
}

/// The tap bank at one lattice ratio, for a source at a rate of its own.
///
/// The taps are the measured ones where the ratio is the measured ratio, whatever
/// pair of rates spells it, and the same shape at that ratio's own [`cutoff`]
/// otherwise. Build one per resampling run: a derived bank is computed on
/// construction, not per field.
pub struct Kernel {
    num: u32,
    den: u32,
    bank: Option<Box<[[f32; TAPS]; PHASES]>>,
}

impl Kernel {
    /// The kernel for a lattice of `num` source samples per `den` fields — a source
    /// rate and a target rate, in that order, or any ratio equal to theirs.
    pub fn new(num: u32, den: u32) -> Kernel {
        let cutoff = cutoff(num, den);
        Kernel {
            num,
            den,
            bank: (cutoff != B).then(|| closed_form_at(cutoff)),
        }
    }

    fn taps(&self) -> &[[f32; TAPS]; PHASES] {
        self.bank.as_deref().unwrap_or_else(|| taps())
    }

    /// Sum field `f`'s tap products in `f64`; samples outside `source` are zero.
    pub fn accumulate(&self, source: &[i16], field: usize) -> f64 {
        accumulate_over(self.taps(), source, field, self.num, self.den)
    }

    /// Return a field in source units, truncating toward zero once after the full sum.
    pub fn field(&self, source: &[i16], at: usize) -> i64 {
        self.accumulate(source, at).trunc() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::super::codec::{FIELD_RATE, SOURCE_RATE};
    use super::*;

    #[test]
    fn the_bessel_series_matches_tabulated_values() {
        assert_eq!(bessel_i0(0.0), 1.0);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008_4).abs() < 1e-15);
        assert!((bessel_i0(8.0) - 427.564_115_721_804_74).abs() < 1e-12);
    }

    #[test]
    fn the_support_is_half_open() {
        let edge = (B * sinc(B * 15.0) / bessel_i0(BETA)) as f32;
        assert_eq!(taps()[0][0], edge);
        assert!((f64::from(edge) - 4.79e-5).abs() < 1e-7, "{edge}");
        assert_eq!(h_at(15.0, B), 0.0);
        assert_eq!(h_at(-15.0 - f64::EPSILON * 16.0, B), 0.0);
        assert_ne!(h_at(-15.0, B), 0.0);
    }

    #[test]
    fn the_closed_form_is_mirror_symmetric_to_the_bit() {
        let bank = closed_form();
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

    #[test]
    fn the_measured_taps_stay_within_a_rounding_of_the_closed_form() {
        let mut points: Vec<(usize, usize)> = MEASURED.iter().map(|&(p, t, _)| (p, t)).collect();
        points.sort_unstable();
        points.dedup();
        assert_eq!(points.len(), MEASURED.len());
        for &(phase, tap, value) in &MEASURED {
            assert!(phase < PHASES && tap < TAPS, "phase {phase} tap {tap}");
            let ideal = h_at(tap as f64 - FIRST as f64 + phase as f64 / PHASES as f64, B);
            let off = (f64::from(value) - ideal).abs();
            assert!(off < 2e-7, "phase {phase} tap {tap}: {value} vs {ideal}");
            assert_ne!(value, ideal as f32, "phase {phase} tap {tap}");
        }
    }

    #[test]
    fn every_phase_sums_near_unity() {
        for (phase, row) in taps().iter().enumerate() {
            let sum: f64 = row.iter().map(|&g| f64::from(g)).sum();
            assert!((sum - 1.0).abs() < 1.5e-4, "phase {phase}: {sum}");
        }
    }

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

    #[test]
    fn products_are_single_precision() {
        let mut source = vec![0i16; 64];
        source[0] = 32767;
        let sample = f32::from(32767i16) * DEPTH_16;
        let expected = f64::from(sample * taps()[0][FIRST as usize]);
        assert_eq!(accumulate(&source, 0), expected);
        assert_ne!(
            expected,
            f64::from(sample) * f64::from(taps()[0][FIRST as usize])
        );
    }

    #[test]
    fn another_ratio_walks_the_same_bank() {
        assert_eq!(lattice_at(7, PITCH_NUM, PITCH_DEN), lattice(7));
        // Two source samples per field lands on a stored sample every time.
        for f in 0..8 {
            assert_eq!(lattice_at(f, 2, 1), (2 * f as i128, 0));
        }
        // Three source samples per two fields alternates whole and half.
        assert_eq!(lattice_at(1, 3, 2), (1, PHASES / 2));
        assert_eq!(lattice_at(2, 3, 2), (3, 0));
    }

    /// The cutoff follows the ratio, and at the measured ratio it is the measured
    /// value to the bit — however the caller spells that ratio — so the lattice the
    /// bank was measured on runs the measured taps and nothing else does.
    #[test]
    fn the_cutoff_keeps_the_narrower_nyquist() {
        assert_eq!(cutoff(PITCH_NUM, PITCH_DEN), B);
        assert_eq!(cutoff(SOURCE_RATE, FIELD_RATE), B);
        assert!(Kernel::new(SOURCE_RATE, FIELD_RATE).bank.is_none());

        // A faster source keeps the same band, which is a smaller part of its own.
        let faster = cutoff(96_000, FIELD_RATE);
        assert!(faster < B, "{faster}");
        assert!((faster * 48_000.0 - B * f64::from(SOURCE_RATE) / 2.0).abs() < 1e-6);

        // A slower source keeps its whole band: the cutoff sits over its own Nyquist.
        let slower = cutoff(22_050, FIELD_RATE);
        assert!(slower > 1.0, "{slower}");
        assert!(Kernel::new(22_050, FIELD_RATE).bank.is_some());
    }

    /// A field on the measured lattice is the same field whichever entry point asks
    /// for it.
    #[test]
    fn the_kernel_at_the_measured_ratio_is_the_free_function() {
        let source: Vec<i16> = (0..512).map(|n| ((n * 37) % 9001 - 4500) as i16).collect();
        let kernel = Kernel::new(PITCH_NUM, PITCH_DEN);
        for f in 0..300 {
            assert_eq!(kernel.field(&source, f), field(&source, f), "field {f}");
        }
    }

    #[test]
    fn the_phase_truncates_the_fraction() {
        assert_eq!(lattice(0), (0, 0));
        // t(1) = 22050/17501 = 1 + 4549/17501; 4549·512/17501 = 133.08.
        assert_eq!(lattice(1), (1, 133));
        // t(17501) = 22050 exactly.
        assert_eq!(lattice(17501), (22050, 0));
    }

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
