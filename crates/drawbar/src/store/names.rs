//! Filenames in a library: the rule every name drawbar creates follows, and the key two
//! names collide under.
//!
//! A library can be copied between macOS, Windows and Linux, so the rule is the
//! strictest of the three. APFS and NTFS compare names without case, ext4 compares bytes,
//! and a name one of them cannot hold is refused before anything is written.

/// The characters Windows forbids in a filename, which also covers the separators of
/// every supported system.
const FORBIDDEN: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// The names Windows keeps for devices, refused in any case and with any extension.
const DEVICES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The longest name a component may be, in bytes of UTF-8. ext4 and APFS stop at 255.
const LONGEST: usize = 255;

/// Why `name` cannot be created in a library, or `None` when every supported system can
/// hold it.
pub fn refusal(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("it is empty");
    }
    if name == "." || name == ".." {
        return Some("it names a folder's link to itself or its parent");
    }
    if name.contains(FORBIDDEN) {
        return Some("it holds a character Windows forbids: / \\ : * ? \" < > |");
    }
    if name.chars().any(char::is_control) {
        return Some("it holds a control character");
    }
    if name.starts_with(' ') || name.ends_with(' ') {
        return Some("it starts or ends with a space");
    }
    if name.starts_with('.') {
        return Some("it starts with a dot, which hides it");
    }
    if name.ends_with('.') {
        return Some("it ends with a dot, which Windows drops");
    }
    if is_device(name) {
        return Some("Windows keeps that name for a device");
    }
    if name.len() > LONGEST {
        return Some("it is longer than 255 bytes");
    }
    None
}

fn is_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    DEVICES
        .iter()
        .any(|device| device.eq_ignore_ascii_case(stem))
}

/// What two names in one folder are compared by: the name lowercased by Unicode's
/// mapping.
///
/// ⚠️ No normalization form is applied. `é` written precomposed and written as `e` plus
/// a combining accent are two keys here, though APFS holds only one of them per folder;
/// on APFS the create itself then refuses the second, and on ext4 both are written.
pub fn key(name: &str) -> String {
    name.to_lowercase()
}

/// `wanted`, or the first of `wanted 2`, `wanted 3`, … that `taken` refuses, with the
/// number before the extension so `c3 2.wav` still reads as a WAV.
pub fn free(wanted: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(&key(wanted)) {
        return wanted.to_string();
    }
    let (stem, tail) = split(wanted);
    (2u64..)
        .map(|nth| format!("{stem} {nth}{tail}"))
        .find(|name| !taken(&key(name)))
        .unwrap_or_else(|| wanted.to_string())
}

/// A name as its stem and its extension, the extension keeping its dot.
fn split(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(at) if at > 0 => name.split_at(at),
        _ => (name, ""),
    }
}

/// A name the app chose, made into one the rule allows: the characters the rule forbids
/// become `-`, control characters go, spaces and dots are trimmed from the ends, and a
/// device name gains a leading `_`. A name with nothing left is `unnamed`.
///
/// For names nobody typed. A name the user typed is refused with its [`refusal`] instead.
pub fn portable(wanted: &str) -> String {
    let folded: String = wanted
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match FORBIDDEN.contains(&c) {
            true => '-',
            false => c,
        })
        .collect();
    let trimmed = folded.trim_matches([' ', '.']);
    let named = match trimmed.is_empty() {
        true => "unnamed".to_string(),
        false => trimmed.to_string(),
    };
    let named = match is_device(&named) {
        true => format!("_{named}"),
        false => named,
    };
    shortened(named)
}

/// `name` cut to [`LONGEST`] bytes, keeping its extension and a whole last character.
fn shortened(name: String) -> String {
    if name.len() <= LONGEST {
        return name;
    }
    let (stem, tail) = split(&name);
    let room = LONGEST.saturating_sub(tail.len());
    let mut end = room.min(stem.len());
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    let cut = stem[..end].trim_end_matches([' ', '.']);
    format!("{cut}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_every_system_holds_is_allowed() {
        for name in [
            "My Piano.npno",
            "c3.wav",
            "Strings",
            "Grand (bright) v2.nsmp",
            "Flügel.npno",
            "CONSOLE.ne5p",
            "COM10.ne5p",
        ] {
            assert_eq!(refusal(name), None, "{name}");
        }
    }

    #[test]
    fn a_windows_device_name_is_refused_in_any_case_and_with_any_extension() {
        for name in [
            "CON",
            "con.wav",
            "Nul.tar.gz",
            "com1.ne5p",
            "LPT9.npno",
            "aux .txt",
        ] {
            assert_eq!(
                refusal(name),
                Some("Windows keeps that name for a device"),
                "{name}"
            );
        }
    }

    #[test]
    fn a_name_some_system_cannot_hold_is_refused_with_its_reason() {
        for (name, why) in [
            ("", "empty"),
            ("..", "link"),
            ("a/b.wav", "character"),
            ("a:b.wav", "character"),
            ("what?.wav", "character"),
            ("tab\there.wav", "control"),
            (" c3.wav", "space"),
            ("c3.wav ", "space"),
            (".hidden.wav", "dot"),
            ("c3.", "dot"),
        ] {
            let said = refusal(name).unwrap_or_else(|| panic!("{name:?} was allowed"));
            assert!(said.contains(why), "{name:?}: {said}");
        }
        assert!(refusal(&"a".repeat(256)).is_some(), "256 bytes");
        assert!(refusal(&"a".repeat(255)).is_none(), "255 bytes");
    }

    #[test]
    fn two_names_that_differ_only_in_case_collide() {
        assert_eq!(key("C3.WAV"), key("c3.wav"));
        assert_eq!(key("Flügel.npno"), key("FLÜGEL.NPNO"));
        assert_ne!(key("Strings.nsmp"), key("Strings.npno"));
        assert_ne!(key("My Piano.npno"), key("My-Piano.npno"));
    }

    #[test]
    fn a_taken_name_is_numbered_before_its_extension() {
        let taken = ["c3.wav", "c3 2.wav", "strings"];
        let is_taken = |key: &str| taken.contains(&key);
        assert_eq!(free("C3.wav", is_taken), "C3 3.wav");
        assert_eq!(free("Strings", is_taken), "Strings 2");
        assert_eq!(free("Grand.npno", is_taken), "Grand.npno");
    }

    #[test]
    fn a_name_the_app_chose_is_made_one_the_rule_allows() {
        for (wanted, made) in [
            ("Africa Split.ne5p", "Africa Split.ne5p"),
            ("A/B: mix?.ne5p", "A-B- mix-.ne5p"),
            ("  .Grand.  ", "Grand"),
            ("CON.ne5p", "_CON.ne5p"),
            ("...", "unnamed"),
            ("tab\there", "tabhere"),
        ] {
            let made_here = portable(wanted);
            assert_eq!(made_here, made, "{wanted:?}");
            assert_eq!(refusal(&made_here), None, "{made_here:?}");
        }
        let long = portable(&format!("{}.npno", "é".repeat(200)));
        assert!(long.len() <= LONGEST, "{}", long.len());
        assert!(long.ends_with(".npno"), "it keeps its extension");
        assert_eq!(refusal(&long), None);
    }
}
