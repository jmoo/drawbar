//! A text note: an asset whose bytes are words, edited as the text they are.
//!
//! [`read`] is the one place that decides which bytes are a note.

use eframe::egui;
use nord_format::util::FileType;

use super::controls::Sets;
use crate::workspace::LocalEntity;

/// The extension a new note is named with, which is what a reader and an export expect
/// to see on one.
pub const EXTENSION: &str = "txt";

/// The most a note holds.
///
/// ⚠️ The editor lays out every byte of a note every frame, and every edit re-reads the
/// whole file. A set list is a few kilobytes; a larger file is a log or a dump, and it
/// stays a record rather than becoming a box that stalls with each keystroke.
pub const MAX_BYTES: usize = 256 * 1024;

/// The one thing this document sets, under the name [`apply`] takes it by.
const TEXT: &str = "text";

const FONT: f32 = 12.5;

const HINT: &str = "Set lists, cues, patch notes — whatever needs writing down.";

/// Why bytes are not a note.
#[derive(Debug, PartialEq, Eq)]
pub enum NotText {
    /// They open the way a format this app decodes does, so a failed decode of them is
    /// that format's error.
    Claimed(&'static str),
    TooLong(usize),
    NotUtf8,
    Control(char),
}

impl std::fmt::Display for NotText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotText::Claimed(format) => {
                write!(f, "a note cannot begin the way a {format} file does")
            }
            NotText::TooLong(held) => write!(
                f,
                "a note holds at most {} KiB, and this is {held} bytes",
                MAX_BYTES / 1024
            ),
            NotText::NotUtf8 => write!(f, "a note is UTF-8"),
            NotText::Control(c) => write!(f, "a note does not hold U+{:04X}", u32::from(*c)),
        }
    }
}

/// The words these bytes are, or why they are not a note.
///
/// UTF-8 of at most [`MAX_BYTES`], holding no control character but tab and the two
/// line breaks, and not opening with the magic of a format `nord-format` decodes. An
/// empty file is a note.
pub fn read(bytes: &[u8]) -> Result<&str, NotText> {
    if bytes.len() > MAX_BYTES {
        return Err(NotText::TooLong(bytes.len()));
    }
    if let Some(format) = claimed(bytes) {
        return Err(NotText::Claimed(format));
    }
    let words = std::str::from_utf8(bytes).map_err(|_| NotText::NotUtf8)?;
    match words.chars().find(|c| !written(*c)) {
        Some(c) => Err(NotText::Control(c)),
        None => Ok(words),
    }
}

pub fn is_text(bytes: &[u8]) -> bool {
    read(bytes).is_ok()
}

/// Whether a note may hold `c`.
fn written(c: char) -> bool {
    !c.is_control() || matches!(c, '\t' | '\n' | '\r')
}

/// The format whose magic opens these bytes, among those `nord-format` decodes.
fn claimed(bytes: &[u8]) -> Option<&'static str> {
    let peeked = nord_format::util::peek(&mut std::io::Cursor::new(bytes)).ok()?;
    match peeked.file_type {
        // Recognised, and decoded by nothing: words that open with `<` stay words.
        FileType::Xml => None,
        FileType::Cbin => Some("CBIN"),
        FileType::Cne3 => Some("CNE3"),
        FileType::Midi => Some("MIDI"),
        FileType::SampleProject => Some("Sample Editor project"),
        FileType::Sysex => Some("SysEx"),
        FileType::Zip => Some("ZIP"),
    }
}

/// How many lines are written here, as an editor counts them: a file with nothing in it
/// is one line, and the newline that ends the last line does not open another.
pub fn lines(words: &str) -> usize {
    words.lines().count().max(1)
}

/// The bytes a note holds once this frame's sets are in them.
///
/// A control character other than tab and the line breaks is dropped from what was
/// typed or pasted. Text that is still not a note is refused.
pub fn apply(bytes: &[u8], sets: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut out = bytes.to_vec();
    for (path, value) in sets {
        match path.as_str() {
            TEXT => {
                out = value
                    .chars()
                    .filter(|c| written(*c))
                    .collect::<String>()
                    .into()
            }
            _ => return Err(format!("unknown field {path:?}")),
        }
    }
    read(&out).map_err(|why| why.to_string())?;
    Ok(out)
}

/// The editor's buffer, kept across frames so a frame copies nothing.
#[derive(Default)]
pub struct State {
    text: String,
}

impl State {
    /// The whole page: one box, the size of the room under the header.
    ///
    /// ⚠️ The box scrolls itself. It is the whole page, so the page has nothing left to
    /// scroll, and a caret pushed past the bottom has to move the text rather than the
    /// document around it.
    pub fn ui(&mut self, ui: &mut egui::Ui, entity: &LocalEntity, sets: &mut Sets) {
        // A refused edit leaves the buffer holding what the file does not, and this is
        // what puts the file's words back.
        if self.text.as_bytes() != entity.bytes.as_slice() {
            match read(&entity.bytes) {
                Ok(words) => words.clone_into(&mut self.text),
                Err(why) => {
                    ui.label(
                        egui::RichText::new(why.to_string()).color(crate::app::bad(ui.visuals())),
                    );
                    return;
                }
            }
        }
        let font = egui::FontId::monospace(FONT);
        let room = ui.available_size();
        let filled = rows(room.y, ui.fonts(|fonts| fonts.row_height(&font)));
        let written = egui::ScrollArea::vertical()
            .id_salt(TEXT)
            .auto_shrink([false; 2])
            .max_height(room.y)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.text)
                        .font(font)
                        .hint_text(HINT)
                        .frame(false)
                        .margin(egui::Margin::ZERO)
                        .desired_width(f32::INFINITY)
                        .desired_rows(filled)
                        .lock_focus(true),
                )
            })
            .inner;
        if written.changed() {
            sets.push((TEXT.to_string(), self.text.clone()));
        }
    }
}

/// How many rows of `row` fill `room`, which is what makes the box the size of the room
/// it is in. Never fewer than one: a window too short for a line still edits.
fn rows(room: f32, row: f32) -> usize {
    match row > 0.0 {
        true => ((room / row).floor() as usize).max(1),
        false => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_is_utf8_holding_only_the_control_characters_text_is_written_with() {
        for words in [
            &b""[..],
            b"Set 1\n  1. One More Time\r\n\tdrop D\n",
            "café — naïve, 音楽".as_bytes(),
            b"\xef\xbb\xbfwith a byte order mark",
            b"<intro> is XML's first byte, and nothing decodes XML",
            b"SMAC is short of the project magic",
        ] {
            assert!(is_text(words), "{:?}", String::from_utf8_lossy(words));
        }
        for (held, why) in [
            (&b"caf\xe9 in latin-1"[..], NotText::NotUtf8),
            (&[0xff, 0xfe, 0x53, 0x00][..], NotText::NotUtf8),
            (b"a bell \x07", NotText::Control('\u{7}')),
            (b"a form feed \x0c", NotText::Control('\u{c}')),
            ("C1 \u{0085}".as_bytes(), NotText::Control('\u{85}')),
        ] {
            assert_eq!(read(held), Err(why), "{:?}", String::from_utf8_lossy(held));
        }
    }

    #[test]
    fn bytes_under_a_decoded_formats_magic_are_never_a_note() {
        for (held, format) in [
            (
                &b"SMACEditorProject {\n  m_fileFormatVersion = 99\n}\n"[..],
                "Sample Editor project",
            ),
            (b"CBIN is where a program lives", "CBIN"),
            (b"MThd", "MIDI"),
        ] {
            assert_eq!(
                read(held),
                Err(NotText::Claimed(format)),
                "{:?}",
                String::from_utf8_lossy(held)
            );
        }
    }

    #[test]
    fn a_note_is_bounded_in_size() {
        let most = "a".repeat(MAX_BYTES);
        assert!(is_text(most.as_bytes()));
        let over = "a".repeat(MAX_BYTES + 1);
        assert_eq!(read(over.as_bytes()), Err(NotText::TooLong(MAX_BYTES + 1)));
    }

    #[test]
    fn a_line_count_is_what_an_editor_would_call_one() {
        assert_eq!(lines(""), 1);
        assert_eq!(lines("one"), 1);
        assert_eq!(lines("one\n"), 1);
        assert_eq!(lines("one\ntwo"), 2);
        assert_eq!(lines("one\n\n"), 2);
    }

    #[test]
    fn an_edit_writes_the_text_it_was_given() {
        let out = apply(b"Set 1\n", &[(TEXT.into(), "Set 1\nSet 2\n".into())]).unwrap();
        assert_eq!(out, b"Set 1\nSet 2\n");
    }

    #[test]
    fn pasted_control_characters_are_dropped_and_the_note_stays_text() {
        let pasted = "Set\u{1b}[1m 1\0\u{0c}\u{85}\n\tcue\r\n";
        let out = apply(b"", &[(TEXT.into(), pasted.into())]).unwrap();
        assert_eq!(out, b"Set[1m 1\n\tcue\r\n");
        assert!(is_text(&out));
    }

    #[test]
    fn an_edit_that_would_not_be_a_note_is_refused() {
        let long = "a".repeat(MAX_BYTES + 1);
        let err = apply(b"Set 1\n", &[(TEXT.into(), long)]).unwrap_err();
        assert!(err.contains("at most"), "{err}");
        let err = apply(b"", &[(TEXT.into(), "SMACEditorProject {".into())]).unwrap_err();
        assert!(err.contains("Sample Editor project"), "{err}");
    }

    #[test]
    fn an_unknown_set_is_refused() {
        let err = apply(b"Set 1\n", &[("name".into(), "elsewhere".into())]).unwrap_err();
        assert!(err.contains("name"), "{err}");
        assert_eq!(apply(b"Set 1\n", &[]).unwrap(), b"Set 1\n");
    }

    #[test]
    fn the_box_fills_the_room_it_is_in() {
        assert_eq!(rows(300.0, 15.0), 20);
        assert_eq!(rows(305.0, 15.0), 20, "a part row would overflow the page");
        assert_eq!(rows(0.0, 15.0), 1);
        assert_eq!(rows(-10.0, 15.0), 1);
        assert_eq!(rows(300.0, 0.0), 1);
    }
}
