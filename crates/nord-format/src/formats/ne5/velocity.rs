//! How an Electro 5 maps a note-on's velocity onto the velocity scale a piano library
//! ([`crate::formats::npno`]) selects its layers on. Other instruments may map it
//! differently.

use crate::types::RangedU8;

/// The piano section's KBD Touch
/// ([`PianoPanel::touch`](super::program::PianoPanel::touch)): each setting above 0 lets
/// a softer note reach a soft layer.
pub type Touch = RangedU8<3>;

/// The highest velocity that reaches each threshold on the velocity scale under each
/// [`Touch`] on an Electro 5, for thresholds 0, [`REACH_STEP`], twice that, and so on,
/// measured at every second velocity. The reach follows the instrument's velocity curve
/// at about six thresholds a decibel and flattens where the curve does at low velocities.
/// At KBD Touch 0, 205 is past reach at velocity 1. Confirmed on hardware.
const REACH: [[u8; 21]; 4] = [
    [
        127, 107, 105, 99, 93, 85, 79, 73, 67, 61, 57, 53, 51, 45, 43, 39, 33, 31, 27, 23, 21,
    ],
    [
        127, 111, 105, 99, 93, 83, 75, 67, 61, 55, 51, 47, 43, 39, 37, 31, 29, 25, 21, 17, 15,
    ],
    [
        127, 105, 99, 91, 83, 75, 67, 61, 55, 51, 47, 43, 39, 37, 31, 29, 25, 21, 17, 15, 11,
    ],
    [
        127, 97, 91, 83, 75, 67, 61, 55, 51, 47, 43, 41, 37, 33, 31, 29, 25, 21, 17, 15, 11,
    ],
];
const REACH_STEP: u16 = 10;

/// The highest threshold on the velocity scale any note-on reaches on an Electro 5, at
/// velocity 1.
pub const HIGHEST_THRESHOLD: u16 = (REACH[0].len() as u16 - 1) * REACH_STEP;

/// The highest velocity whose note-on reaches `threshold` on the velocity scale under
/// `touch`, or 0 where none does.
///
/// A key plays the stroke of its root and bank with the largest threshold its velocity
/// reaches, and of strokes with one threshold the last. The threshold is
/// [`Stroke::threshold`](crate::formats::npno::Stroke::threshold) in a 6.x library and
/// [`Version::threshold`](crate::formats::npno::Version::threshold) in a 5.x one. So a
/// layer plays from 1 up to this velocity, less what softer layers take. Measured every
/// ten thresholds, interpolated between them, and within two velocities of where a layer
/// switches. KBD Touch changes where the layers switch, not
/// how loud velocity 127 plays. Confirmed on hardware.
pub fn top_velocity(threshold: u16, touch: Touch) -> u8 {
    if threshold > HIGHEST_THRESHOLD {
        return 0;
    }
    let reach = &REACH[usize::from(u8::from(touch))];
    let step = usize::from(threshold / REACH_STEP);
    let into = threshold % REACH_STEP;
    let low = reach[step];
    let high = reach.get(step + 1).copied().unwrap_or(low);
    low - (u16::from(low - high) * into).div_ceil(REACH_STEP) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The highest velocity measured to reach each tenth threshold, and the interpolation
    /// between them; nothing reaches past 200, where velocity 1 stops.
    #[test]
    fn a_threshold_is_reached_up_to_the_velocity_the_instrument_was_heard_at() {
        let touch = |t: u8| Touch::try_from(t).unwrap();
        let measured = [
            (0, 0, 127),
            (0, 10, 107),
            (0, 50, 85),
            (0, 100, 57),
            (0, 150, 39),
            (0, 200, 21),
            (1, 100, 51),
            (2, 100, 47),
            (3, 100, 43),
            (3, 200, 11),
        ];
        for (kbd, threshold, velocity) in measured {
            assert_eq!(
                top_velocity(threshold, touch(kbd)),
                velocity,
                "threshold {threshold} at KBD Touch {kbd}"
            );
        }
        assert_eq!(top_velocity(34, touch(0)), 96);
        for kbd in 0..=3 {
            assert_eq!(top_velocity(HIGHEST_THRESHOLD + 1, touch(kbd)), 0);
            let reach: Vec<u8> = (0..=HIGHEST_THRESHOLD)
                .map(|threshold| top_velocity(threshold, touch(kbd)))
                .collect();
            assert!(
                reach.windows(2).all(|pair| pair[0] >= pair[1]),
                "a higher threshold is never reached at a higher velocity: {reach:?}"
            );
        }
    }
}
