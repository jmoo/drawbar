//! IIR filters as second-order sections, run forward only.
//!
//! Run both ways, a notch rings ahead of each attack; forward only, nothing comes before
//! the note.

use std::f64::consts::PI;

/// One second-order section, normalized so its first feedback coefficient is one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    pub b: [f64; 3],
    pub a: [f64; 2],
}

/// A Butterworth high-pass of even `order` at `hz`, as sections. The cutoff is
/// pre-warped, so the response is 3 dB down at `hz` as an analog one would be.
///
/// # Panics
///
/// If `order` is odd or zero, or `hz` is not between zero and half of `rate`.
pub fn highpass(order: usize, hz: f64, rate: u32) -> Vec<Section> {
    let nyquist = f64::from(rate) / 2.0;
    assert!(order > 0 && order.is_multiple_of(2), "an even order");
    assert!(
        hz > 0.0 && hz < nyquist,
        "a cutoff under the Nyquist frequency"
    );
    let w0 = 2.0 * PI * hz / f64::from(rate);
    (0..order / 2)
        .map(|k| {
            let q = 1.0 / (2.0 * ((2 * k + 1) as f64 * PI / (2 * order) as f64).sin());
            let alpha = w0.sin() / (2.0 * q);
            let cos = w0.cos();
            let a0 = 1.0 + alpha;
            let b0 = (1.0 + cos) / 2.0 / a0;
            Section {
                b: [b0, -2.0 * b0, b0],
                a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            }
        })
        .collect()
}

/// A notch at `hz` whose -3 dB width is `hz / q`.
pub fn notch(hz: f64, q: f64, rate: u32) -> Section {
    let w0 = 2.0 * PI * hz / f64::from(rate);
    let beta = (w0 / q / 2.0).tan();
    let gain = 1.0 / (1.0 + beta);
    let cos = w0.cos();
    Section {
        b: [gain, -2.0 * gain * cos, gain],
        a: [-2.0 * gain * cos, 2.0 * gain - 1.0],
    }
}

/// `x` through every section in turn, from rest.
pub fn run(sections: &[Section], x: &[f64]) -> Vec<f64> {
    let mut y = x.to_vec();
    for s in sections {
        let (mut z0, mut z1) = (0.0, 0.0);
        for v in &mut y {
            let input = *v;
            let out = s.b[0] * input + z0;
            z0 = s.b[1] * input - s.a[0] * out + z1;
            z1 = s.b[2] * input - s.a[1] * out;
            *v = out;
        }
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gain at `hz` over the last half of 10 s, once the narrowest notch has settled.
    fn gain_db(sections: &[Section], hz: f64, rate: u32) -> f64 {
        let n = rate as usize * 10;
        let x: Vec<f64> = (0..n)
            .map(|i| (2.0 * PI * hz * i as f64 / f64::from(rate)).sin())
            .collect();
        let y = run(sections, &x);
        let rms = |v: &[f64]| (v.iter().map(|s| s * s).sum::<f64>() / v.len() as f64).sqrt();
        20.0 * (rms(&y[n / 2..]) / rms(&x[n / 2..])).log10()
    }

    #[test]
    fn a_butterworth_high_pass_is_3_db_down_at_its_cutoff_and_falls_at_its_order() {
        for order in [2, 4] {
            let sections = highpass(order, 100.0, 35_002);
            assert!((gain_db(&sections, 100.0, 35_002) + 3.01).abs() < 0.05);
            assert!(gain_db(&sections, 1_000.0, 35_002).abs() < 0.01);
            let octave_down = gain_db(&sections, 25.0, 35_002) - gain_db(&sections, 50.0, 35_002);
            assert!(
                (octave_down + 6.02 * order as f64).abs() < 0.5,
                "{octave_down}"
            );
        }
    }

    #[test]
    fn a_notch_takes_out_its_line_and_leaves_its_neighbors() {
        let line = notch(150.0, 150.0 / 0.66, 35_002);
        assert!(gain_db(&[line], 150.0, 35_002) < -40.0);
        assert!(gain_db(&[line], 170.0, 35_002).abs() < 0.1);
    }
}
