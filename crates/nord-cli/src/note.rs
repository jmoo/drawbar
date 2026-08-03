//! MIDI note names, for zone display and edit values.
//!
//! Middle C (60) is spelled C4, matching how the sample editor labels keys — the
//! corpus specimens were named off that display (`D2-rootkey-C3` holds 48).

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
    let octave: i32 = octave
        .parse()
        .map_err(|_| format!("{s:?} has no octave number"))?;
    u8::try_from((octave + 1) * 12 + semitone + accidental)
        .ok()
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

    #[test]
    fn nonsense_is_refused() {
        assert!(parse("128").is_err());
        assert!(parse("H4").is_err());
        assert!(parse("C").is_err());
        assert!(parse("C99").is_err());
        assert!(parse("").is_err());
    }
}
