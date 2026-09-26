//! A WAV that landed in the workspace, and the instrument that can be made from it.
//!
//! A WAV has no document of its own: opened here, it is bytes with an error beside them.
//! `nord_format`'s sample encoder builds a one-zone instrument from 44.1 kHz mono or
//! stereo 16-bit PCM in any of the three generations, and this module draws the panel
//! that runs it.
//!
//! ⚠️ Only a v2 instrument has been played on hardware. The wide generations reproduce
//! what Nord Sample Editor renders, but no instrument that plays them has been
//! available, so the panel marks them unverified.

use eframe::egui;
use nord_format::formats::nsmp::codec::{Layout, SOURCE_RATE};
use nord_format::formats::nsmp::{encode, MAX_NAME_LEN};
use nord_format::note;
use nord_format::wav::Pcm16;

use super::controls;
use super::sample::note_picker;

/// Whether these bytes are worth offering an encode panel over.
///
/// Tests the container only: a 24-bit WAV still gets the panel, which says why it
/// cannot be encoded.
pub fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE"
}

/// What the operator has typed into the panel, kept between frames.
pub struct Draft {
    pub name: String,
    pub root_key: u8,
    pub top_note: u8,
    /// Every content field written in full, without the editor's record coding.
    pub plain: bool,
    pub layout: Layout,
}

impl Draft {
    /// A draft named after `label`, at the encoder's defaults: root at middle C, top note
    /// two octaves above, and v2, the only generation played on hardware.
    pub fn new(label: &str) -> Draft {
        let stem = label.rsplit_once('.').map_or(label, |(stem, _)| stem);
        let mut name = stem.to_string();
        controls::fits(&mut name, MAX_NAME_LEN);
        Draft {
            name,
            root_key: 60,
            top_note: 84,
            plain: false,
            layout: Layout::V2,
        }
    }
}

/// The read of a WAV the panel works over, decoded once per set of bytes.
pub enum Source {
    Read(Pcm16),
    Unreadable(String),
}

impl Source {
    pub fn read(bytes: &[u8]) -> Source {
        match nord_format::wav::read_pcm16(bytes) {
            Ok(pcm) => Source::Read(pcm),
            Err(e) => Source::Unreadable(e.to_string()),
        }
    }
}

/// Why this WAV cannot become an instrument, in the operator's words.
///
/// The three limits are the encoder's: the field lattice is defined at one rate, a stroke
/// carries one or two channels, and a stroke shorter than [`encode::MIN_FRAMES`] has an
/// opening the encoder does not model.
pub fn refusal(source: &Source) -> Option<String> {
    let pcm = match source {
        Source::Unreadable(why) => return Some(why.clone()),
        Source::Read(pcm) => pcm,
    };
    if pcm.rate != SOURCE_RATE {
        return Some(format!(
            "{} Hz: the encoder takes only {SOURCE_RATE} Hz, so resample the file first",
            pcm.rate
        ));
    }
    if pcm.channels != 1 && pcm.channels != 2 {
        return Some(format!(
            "{} channels: an instrument's audio holds one or two channels",
            pcm.channels
        ));
    }
    if pcm.frames() < encode::MIN_FRAMES {
        return Some(format!(
            "{} frames: the encoder needs at least {}",
            pcm.frames(),
            encode::MIN_FRAMES
        ));
    }
    None
}

/// The instrument this draft makes out of `source`.
pub fn instrument(draft: &Draft, source: &Source) -> Result<Vec<u8>, String> {
    if let Some(why) = refusal(source) {
        return Err(why);
    }
    let Source::Read(pcm) = source else {
        return Err("this file did not read as a WAV".into());
    };
    let options = encode::Options::new(&draft.name)
        .root_key(draft.root_key)
        .top_note(draft.top_note)
        .channels(pcm.channels)
        .layout(draft.layout)
        .predictor(match draft.plain {
            true => encode::Predictor::Plain,
            false => encode::Predictor::Minimising,
        });
    let instrument = encode::instrument(&pcm.samples, &options).map_err(|e| e.to_string())?;
    instrument.to_bytes().map_err(|e| e.to_string())
}

fn generation_label(layout: Layout) -> &'static str {
    match layout {
        Layout::V2 => "v2",
        Layout::V3 => "v3",
        Layout::V4 => "v4",
    }
}

/// How far each generation has been taken, in the operator's words.
fn generation_note(layout: Layout) -> &'static str {
    match layout {
        Layout::V2 => "played on hardware: mono, stereo and looped",
        Layout::V3 | Layout::V4 => {
            "unverified: this reproduces the Sample Editor's render, but it has not \
             been played on an instrument that supports this generation"
        }
    }
}

/// Draw the panel. `true` once the operator has asked for the instrument to be made.
pub fn ui(ui: &mut egui::Ui, draft: &mut Draft, source: &Source) -> bool {
    ui.label(egui::RichText::new("This is a WAV, not a Nord file.").strong());
    ui.label(
        egui::RichText::new(
            "It can be encoded into a one-zone sample instrument. The file matches Nord \
             Sample Editor's output apart from floating-point rounding in the resampling \
             kernel, which changes nothing the instrument plays. Instruments encoded \
             this way have been played on hardware.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    if let Source::Read(pcm) = source {
        ui.label(
            egui::RichText::new(format!(
                "{} Hz, {}, {} frames ({:.3} s)",
                pcm.rate,
                match pcm.channels {
                    1 => "mono".to_string(),
                    2 => "stereo".to_string(),
                    n => format!("{n} channels"),
                },
                pcm.frames(),
                pcm.frames() as f64 / f64::from(pcm.rate).max(1.0),
            ))
            .small()
            .weak(),
        );
    }

    let refusal = refusal(source);
    ui.add_enabled_ui(refusal.is_none(), |ui| {
        ui.horizontal(|ui| {
            ui.add_sized(
                [120.0, ui.spacing().interact_size.y],
                egui::Label::new("Name").halign(egui::Align::LEFT),
            );
            ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(200.0));
            controls::fits(&mut draft.name, MAX_NAME_LEN);
        });
        ui.horizontal(|ui| {
            ui.add_sized(
                [120.0, ui.spacing().interact_size.y],
                egui::Label::new("Zone").halign(egui::Align::LEFT),
            );
            ui.label("root key");
            if let Some(note) = note_picker(ui, ("encode_root", 0), draft.root_key) {
                draft.root_key = note;
            }
            ui.label("top note");
            if let Some(note) = note_picker(ui, ("encode_top", 0), draft.top_note) {
                draft.top_note = note;
            }
        });
        ui.horizontal(|ui| {
            ui.add_sized(
                [120.0, ui.spacing().interact_size.y],
                egui::Label::new("Generation").halign(egui::Align::LEFT),
            );
            for layout in [Layout::V2, Layout::V3, Layout::V4] {
                ui.selectable_value(&mut draft.layout, layout, generation_label(layout))
                    .on_hover_text(generation_note(layout));
            }
        });
        if draft.layout != Layout::V2 {
            ui.horizontal(|ui| {
                ui.add_space(120.0);
                ui.label(
                    egui::RichText::new(generation_note(draft.layout))
                        .small()
                        .color(crate::app::warn(ui.visuals())),
                );
            });
        }
        ui.horizontal(|ui| {
            ui.add_space(120.0);
            ui.checkbox(&mut draft.plain, "Plain records")
                .on_hover_text(
                    "write every content field in full instead of using the editor's \
                     record coding: the same audio in a larger file that differs from \
                     the editor's bytes",
                );
        });
    });

    if let Some(why) = &refusal {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(why).color(crate::app::bad(ui.visuals())));
        return false;
    }
    ui.add_space(8.0);
    let mut asked = false;
    ui.horizontal(|ui| {
        asked = ui
            .button("Encode")
            .on_hover_text("adds a new sample instrument to this computer; the WAV stays as it is")
            .clicked();
        ui.label(
            egui::RichText::new(format!(
                "one zone, root {}, up to {}, .{}",
                note::name(draft.root_key),
                note::name(draft.top_note),
                draft.layout.extension()
            ))
            .small()
            .weak(),
        );
    });
    asked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, channels: u16, frames: usize) -> Vec<u8> {
        let samples = vec![0i16; frames * usize::from(channels)];
        nord_format::wav::pcm16(&samples, rate, channels).unwrap()
    }

    #[test]
    fn only_a_riff_wave_container_offers_the_panel() {
        assert!(is_wav(&wav(44_100, 1, 8)));
        assert!(!is_wav(b"RIFF"));
        assert!(!is_wav(b"not a wav at all"));
        assert!(!is_wav(&[]));
    }

    #[test]
    fn every_refusal_says_which_limit_it_hit() {
        let refused = |bytes: Vec<u8>| refusal(&Source::read(&bytes));

        let ok = wav(SOURCE_RATE, 1, encode::MIN_FRAMES);
        assert_eq!(refused(ok), None);

        let slow = refused(wav(22_050, 1, encode::MIN_FRAMES)).expect("the wrong rate");
        assert!(slow.contains("22050 Hz"), "{slow}");
        assert!(slow.contains(&SOURCE_RATE.to_string()), "{slow}");

        assert_eq!(refused(wav(SOURCE_RATE, 2, encode::MIN_FRAMES)), None);
        let wide = refused(wav(SOURCE_RATE, 3, encode::MIN_FRAMES)).expect("three channels");
        assert!(wide.contains("3 channels"), "{wide}");

        let short = refused(wav(SOURCE_RATE, 1, encode::MIN_FRAMES - 1)).expect("too short");
        assert!(short.contains(&encode::MIN_FRAMES.to_string()), "{short}");

        // An unreadable WAV reports the reader's own error.
        let unreadable = refused(b"RIFF\0\0\0\0WAVE".to_vec()).expect("not readable");
        assert!(!unreadable.is_empty());
    }

    #[test]
    fn a_stereo_wav_encodes_to_a_stereo_stroke() {
        let source = Source::read(&wav(SOURCE_RATE, 2, encode::MIN_FRAMES));
        let bytes = instrument(&Draft::new("Pad.wav"), &source).expect("it encodes");
        let entity = nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap();
        let nord_format::Entity::Sample(nord_format::Sample::V2(sample)) = entity else {
            panic!("not a v2 sample");
        };
        let (at, stroke) = sample.stroke_streams()[0];
        let audio = nord_format::formats::nsmp::codec::decode(
            stroke,
            at,
            nord_format::formats::nsmp::codec::Layout::V2,
        )
        .expect("it decodes");
        assert_eq!(audio.channels, 2);
    }

    #[test]
    fn an_encode_makes_the_instrument_the_panel_describes() {
        let source = Source::read(&wav(SOURCE_RATE, 1, encode::MIN_FRAMES));
        let mut draft = Draft::new("Marimba hit.wav");
        assert_eq!(draft.name, "Marimba hit");
        draft.root_key = 48;
        draft.top_note = 60;

        let bytes = instrument(&draft, &source).expect("it encodes");
        let entity = nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap();
        let snapshot = super::super::sample::snapshot(&entity)
            .expect("a sample instrument")
            .expect("it reads");
        assert_eq!(snapshot.name, "Marimba hit");
        assert_eq!(snapshot.generation, "v2");
        assert_eq!(snapshot.zones.len(), 1);
        assert_eq!(snapshot.zones[0].root_key, 48);
        assert_eq!(snapshot.zones[0].top_note, 60);
        assert_eq!(nord_format::to_bytes(&entity).unwrap(), bytes);
    }

    #[test]
    fn the_panel_opens_on_the_played_generation_and_encodes_the_chosen_one() {
        let source = Source::read(&wav(SOURCE_RATE, 1, encode::MIN_FRAMES));
        assert_eq!(Draft::new("Marimba.wav").layout, Layout::V2);

        for (layout, generation) in [(Layout::V2, "v2"), (Layout::V3, "v3"), (Layout::V4, "v4")] {
            let mut draft = Draft::new("Marimba.wav");
            draft.layout = layout;
            let bytes = instrument(&draft, &source).expect("it encodes");
            let entity = nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap();
            let snapshot = super::super::sample::snapshot(&entity)
                .expect("a sample instrument")
                .expect("it reads");
            assert_eq!(snapshot.generation, generation);
        }
    }

    #[test]
    fn a_refused_wav_is_not_encoded_anyway() {
        let source = Source::read(&wav(48_000, 1, encode::MIN_FRAMES));
        assert!(instrument(&Draft::new("x.wav"), &source).is_err());
    }

    #[test]
    fn a_long_filename_opens_the_panel_on_a_name_that_fits() {
        let draft = Draft::new("an extremely long marimba sample name.wav");
        assert_eq!(draft.name.len(), MAX_NAME_LEN);
        let source = Source::read(&wav(SOURCE_RATE, 1, encode::MIN_FRAMES));
        assert!(instrument(&draft, &source).is_ok());
    }

    #[test]
    fn a_name_of_accented_letters_is_cut_by_bytes() {
        let draft = Draft::new(&format!("{}.wav", "é".repeat(MAX_NAME_LEN)));
        assert!(draft.name.len() <= MAX_NAME_LEN, "{:?}", draft.name);
        assert_eq!(draft.name.chars().count(), MAX_NAME_LEN / 2);
        let source = Source::read(&wav(SOURCE_RATE, 1, encode::MIN_FRAMES));
        assert!(instrument(&draft, &source).is_ok());
    }
}
