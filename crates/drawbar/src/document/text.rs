//! The document for a text file: the words, with nothing between them and the bytes.
//!
//! drawbar has no format for a note and needs none. An asset that decoded into nothing
//! is one when its bytes are words, which [`is_text`] is the whole of — every part of
//! the app that has to tell a note from bytes it cannot read asks there rather than
//! keeping a list of extensions.
//!
//! The bytes are the text. An edit is what was typed, it lands on the working copy in
//! the frame it is made, and the file is saved as it stands. So a note is unsaved the
//! moment it holds something its baseline does not, Revert puts the words back, and
//! Save settles them, none of which this page has to know about.

use eframe::egui;

use super::controls::Sets;
use crate::workspace::LocalEntity;

/// The extension a new note is named with, which is what a reader and an export expect
/// to see on one.
pub const EXTENSION: &str = "txt";

/// The one thing this document sets, under the name [`apply`] takes it by.
const TEXT: &str = "text";

const FONT: f32 = 12.5;

const HINT: &str = "Set lists, cues, patch notes — whatever needs writing down.";

/// Whether these bytes are words rather than a format.
///
/// UTF-8 holding no control characters but the three a text file is written with. An
/// empty file is a note waiting to be typed into.
///
/// ⚠️ The one place this is decided. Bytes this app cannot decode are either a note or
/// bytes nothing here can read, and that is the only thing telling them apart.
pub fn is_text(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    !text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
}

/// How many lines are written here, as an editor counts them: a file with nothing in it
/// is one line, and the newline that ends the last line does not open another.
pub fn lines(bytes: &[u8]) -> usize {
    words(bytes).lines().count().max(1)
}

/// The text these bytes hold. Empty for bytes that are not text, which no caller here
/// has: a document is this shape because [`is_text`] said so.
fn words(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap_or_default()
}

/// The bytes a note holds once this frame's sets are in them: what was typed, which is
/// the whole of the file.
pub fn apply(bytes: &[u8], sets: &[(String, String)]) -> Result<Vec<u8>, String> {
    let mut out = bytes.to_vec();
    for (path, value) in sets {
        match path.as_str() {
            TEXT => out = value.clone().into_bytes(),
            _ => return Err(format!("unknown field {path:?}")),
        }
    }
    Ok(out)
}

/// The whole page: one box, the size of the room under the header.
///
/// ⚠️ The box scrolls itself. It is the whole page, so the page has nothing left to
/// scroll, and a caret pushed past the bottom has to move the text rather than the
/// document around it.
pub fn ui(ui: &mut egui::Ui, entity: &LocalEntity, sets: &mut Sets) {
    let mut text = words(&entity.bytes).to_string();
    let font = egui::FontId::monospace(FONT);
    let room = ui.available_size();
    let filled = rows(room.y, ui.fonts(|fonts| fonts.row_height(&font)));
    let written = egui::ScrollArea::vertical()
        .id_salt(TEXT)
        .auto_shrink([false; 2])
        .max_height(room.y)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .font(font)
                    .hint_text(HINT)
                    .frame(false)
                    .margin(egui::Margin::ZERO)
                    .desired_width(f32::INFINITY)
                    .desired_rows(filled),
            )
        })
        .inner;
    if written.changed() {
        sets.push((TEXT.to_string(), text));
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

    /// Words are a note; a format, or anything holding bytes a text file does not, is
    /// not. An empty file is the note a New starts.
    #[test]
    fn a_note_is_utf8_holding_only_the_control_characters_text_is_written_with() {
        for words in [
            &b""[..],
            b"Set 1\n  1. One More Time\r\n\tdrop D\n",
            "café — naïve, 音楽".as_bytes(),
            b"\xef\xbb\xbfwith a byte order mark",
        ] {
            assert!(is_text(words), "{:?}", String::from_utf8_lossy(words));
        }
        for held in [
            &b"CBIN\0\0\0\0ne5p"[..],
            b"caf\xe9 in latin-1",
            &[0xff, 0xfe, 0x53, 0x00][..],
            b"a bell \x07",
            b"a form feed \x0c",
            // C1: valid UTF-8, and not something a note is written with.
            "\u{0085}".as_bytes(),
        ] {
            assert!(!is_text(held), "{:?}", String::from_utf8_lossy(held));
        }
    }

    /// The count the header shows is the count an editor shows.
    #[test]
    fn a_line_count_is_what_an_editor_would_call_one() {
        assert_eq!(lines(b""), 1);
        assert_eq!(lines(b"one"), 1);
        assert_eq!(lines(b"one\n"), 1);
        assert_eq!(lines(b"one\ntwo"), 2);
        assert_eq!(lines(b"one\n\n"), 2);
    }

    /// What was typed is what the file holds, and nothing else moves.
    #[test]
    fn an_edit_writes_the_text_it_was_given() {
        let out = apply(b"Set 1\n", &[(TEXT.into(), "Set 1\nSet 2\n".into())]).unwrap();
        assert_eq!(out, b"Set 1\nSet 2\n");
    }

    /// A set this document does not declare is refused rather than dropped, so a file
    /// cannot be left holding half an edit.
    #[test]
    fn an_unknown_set_is_refused() {
        let err = apply(b"Set 1\n", &[("name".into(), "elsewhere".into())]).unwrap_err();
        assert!(err.contains("name"), "{err}");
        assert_eq!(apply(b"Set 1\n", &[]).unwrap(), b"Set 1\n");
    }

    /// The box is as tall as the room it is in, and a window with no room for a line
    /// still has one to type in.
    #[test]
    fn the_box_fills_the_room_it_is_in() {
        assert_eq!(rows(300.0, 15.0), 20);
        assert_eq!(rows(305.0, 15.0), 20, "a part row would overflow the page");
        assert_eq!(rows(0.0, 15.0), 1);
        assert_eq!(rows(-10.0, 15.0), 1);
        assert_eq!(rows(300.0, 0.0), 1);
    }
}
