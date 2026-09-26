//! MIDI note names, the spelling the formats' key ranges are read and written in.
//!
//! Middle C (60) is spelled C4, as the sample editor labels it.
//! Inferred from specimens; not confirmed on hardware.

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

pub fn name(note: u8) -> String {
    let octave = (note / 12) as i8 - 1;
    format!("{}{octave}", NAMES[(note % 12) as usize])
}

/// A note as an edit value: a name (`C4`, `F#3`, `Bb2`) or a plain number (`60`).
pub fn parse(s: &str) -> Result<u8, String> {
    let t = s.trim();
    if t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return t
            .parse::<u8>()
            .ok()
            .filter(|&n| n <= 127)
            .ok_or_else(|| format!("{s:?} is not a MIDI note (0-127)"));
    }
    let mut chars = t.chars();
    let semitone = match chars.next().map(|c| c.to_ascii_uppercase()) {
        Some('C') => 0i32,
        Some('D') => 2,
        Some('E') => 4,
        Some('F') => 5,
        Some('G') => 7,
        Some('A') => 9,
        Some('B') => 11,
        _ => return Err(format!("{s:?} is not a note name or a number")),
    };
    let rest = chars.as_str();
    let (accidental, octave) = match rest.chars().next() {
        Some('#') => (1, &rest[1..]),
        Some('b') => (-1, &rest[1..]),
        _ => (0, rest),
    };
    // `parse` would also take `+4`, and an octave has one spelling.
    let octave: i32 = octave
        .parse()
        .ok()
        .filter(|_| !octave.starts_with('+'))
        .ok_or_else(|| format!("{s:?} has no octave number"))?;
    // ⚠️ C-1 is note 0 and G9 is 127. A wider octave overflows the sum in a release
    // build, where it wraps into a number that passes the range check below.
    (-1..=9)
        .contains(&octave)
        .then(|| (octave + 1) * 12 + semitone + accidental)
        .and_then(|n| u8::try_from(n).ok())
        .filter(|&n| n <= 127)
        .ok_or_else(|| format!("{s:?} is outside MIDI's 0-127"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for n in 0..=127u8 {
            assert_eq!(parse(&name(n)).unwrap(), n);
        }
    }

    #[test]
    fn middle_c_is_c4() {
        assert_eq!(name(60), "C4");
        assert_eq!(parse("C4").unwrap(), 60);
        assert_eq!(name(0), "C-1");
    }

    #[test]
    fn accidentals_and_numbers() {
        assert_eq!(parse("F#3").unwrap(), 54);
        assert_eq!(parse("Bb2").unwrap(), 46);
        assert_eq!(parse("c4").unwrap(), 60);
        assert_eq!(parse("60").unwrap(), 60);
    }

    /// ⚠️ The octave reaches the note number through a multiplication, so an
    /// unbounded one would wrap in a release build.
    #[test]
    fn an_octave_outside_the_keyboard_is_refused_rather_than_wrapped() {
        for bad in ["C357913941", "C2147483647", "C10", "Cb-1"] {
            let err = parse(bad).unwrap_err();
            assert!(err.contains("outside MIDI's 0-127"), "{bad}: {err}");
        }
        assert!(parse("C+4").unwrap_err().contains("octave"));
    }

    #[test]
    fn nonsense_is_refused() {
        for bad in ["128", "H4", "C", "C99", ""] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
