//! A gain applied to a WAV's samples in the encoding they are stored in, so nothing but
//! the samples changes and an integer file stays the width it was.

use std::ops::{Range, RangeInclusive};

use super::Encoding;

/// The values an integer sample of `bits` holds. 8-bit samples are stored unsigned, and
/// are counted here from their midpoint, as the wider ones are.
fn span(bits: u16) -> RangeInclusive<i64> {
    let half = 1i64 << (bits - 1);
    -half..=half - 1
}

/// `sample`, an integer sample of `bits`, times `factor`, rounded, and whether it was
/// clipped to fit.
pub fn scaled(sample: i32, factor: f64, bits: u16) -> (i32, bool) {
    let exact = (f64::from(sample) * factor).round();
    let span = span(bits);
    let held = exact.clamp(*span.start() as f64, *span.end() as f64);
    (held as i32, held != exact)
}

/// The integer sample stored in `at`, `bits` wide.
fn int(at: &[u8], bits: u16) -> i32 {
    match bits {
        8 => i32::from(at[0]) - 128,
        16 => i32::from(i16::from_le_bytes([at[0], at[1]])),
        24 => i32::from_le_bytes([0, at[0], at[1], at[2]]) >> 8,
        _ => i32::from_le_bytes([at[0], at[1], at[2], at[3]]),
    }
}

/// Store `sample` in `at`, `bits` wide.
fn put_int(at: &mut [u8], sample: i32, bits: u16) {
    match bits {
        8 => at[0] = (sample + 128) as u8,
        16 => at.copy_from_slice(&(sample as i16).to_le_bytes()),
        24 => at.copy_from_slice(&sample.to_le_bytes()[..3]),
        _ => at.copy_from_slice(&sample.to_le_bytes()),
    }
}

/// Each integer sample of `bits` in `data`, a file's data chunk in `bytes`, in file order.
pub fn int_samples(bytes: &[u8], data: Range<usize>, bits: u16) -> impl Iterator<Item = i32> + '_ {
    bytes[data]
        .chunks_exact(usize::from(bits / 8))
        .map(move |at| int(at, bits))
}

/// Scale every sample of `data`, a data chunk's body, by `factor` in `encoding`, and
/// count the samples clipped. Float holds any level, so nothing clips there.
pub fn scale(data: &mut [u8], encoding: Encoding, factor: f64) -> usize {
    let mut clipped = 0;
    match encoding {
        Encoding::Int { bits } => {
            for at in data.chunks_exact_mut(usize::from(bits / 8)) {
                let (held, clips) = scaled(int(at, bits), factor, bits);
                put_int(at, held, bits);
                clipped += usize::from(clips);
            }
        }
        Encoding::Float { bits: 32 } => {
            for at in data.as_chunks_mut::<4>().0 {
                let x = f64::from(f32::from_le_bytes(*at)) * factor;
                *at = (x as f32).to_le_bytes();
            }
        }
        Encoding::Float { .. } => {
            for at in data.as_chunks_mut::<8>().0 {
                *at = (f64::from_le_bytes(*at) * factor).to_le_bytes();
            }
        }
    }
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_integer_width_round_trips_and_clips_at_its_own_range() {
        for bits in [8u16, 16, 24, 32] {
            let width = usize::from(bits / 8);
            let top = *span(bits).end() as i32;
            let low = *span(bits).start() as i32;
            let mut data = vec![0u8; 3 * width];
            for (i, sample) in [top / 4, low / 4, top / 2 + 1].into_iter().enumerate() {
                put_int(&mut data[i * width..(i + 1) * width], sample, bits);
            }
            let read: Vec<i32> = int_samples(&data, 0..data.len(), bits).collect();
            assert_eq!(read, [top / 4, low / 4, top / 2 + 1], "{bits} bits");
            let clipped = scale(&mut data, Encoding::Int { bits }, 2.0);
            let doubled: Vec<i32> = int_samples(&data, 0..data.len(), bits).collect();
            assert_eq!(clipped, 1, "{bits} bits: only the one past half clips");
            assert_eq!(doubled[2], top, "{bits} bits");
        }
    }

    #[test]
    fn a_float_file_scales_and_never_clips() {
        let mut data: Vec<u8> = [0.75f32, -0.5]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(scale(&mut data, Encoding::Float { bits: 32 }, 2.0), 0);
        let back: Vec<f32> = (data.as_chunks::<4>().0.iter())
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        assert_eq!(back, [1.5, -1.0]);
    }
}
