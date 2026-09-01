//! A WAV that landed in the workspace, and the instrument that can be made from it.
//!
//! Nothing decodes a WAV, so one opened here has no document of its own — it is bytes
//! with an error beside them. What it does have is a use: `nord_format`'s sample encoder
//! builds a one-zone v2 instrument out of 44.1 kHz mono 16-bit PCM, and that is the panel
//! this module draws.
//!
//! ⚠️ The encoder is a reconstruction. What it writes is structurally sound and decodes
//! back through that crate's own codec exactly, but it is **not** byte-identical to what
//! Nord Sample Editor writes for the same input. The panel says so where the operator
//! can read it.

use eframe::egui;
use nord_format::formats::nsmp::codec::SOURCE_RATE;
use nord_format::formats::nsmp::{encode, MAX_NAME_LEN};
use nord_format::wav::Pcm16;

use super::sample::note_picker;
use crate::note;

/// Whether these bytes are worth offering an encode panel over.
///
/// The container test alone, not a successful read: a 24-bit WAV is still a WAV, and
/// the panel exists to say why that one cannot be encoded.
pub fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE"
}

/// What the operator has typed into the panel, kept between frames.
pub struct Draft {
    pub name: String,
    pub root_key: u8,
    pub top_note: u8,
    /// Narrowest predictor order per cell rather than every field stated outright.
    pub predict: bool,
}

impl Draft {
    /// A panel over `label`, opened at the encoder's own defaults: middle C, and two
    /// octaves above it.
    pub fn new(label: &str) -> Draft {
        let stem = label.rsplit_once('.').map_or(label, |(stem, _)| stem);
        Draft {
            name: fits(stem),
            root_key: 60,
            top_note: 84,
            predict: false,
        }
    }
}

/// The longest prefix of `label` the name field takes. The limit is in bytes and the
/// cut is on a character boundary, so a name of accented letters loses a letter rather
/// than becoming a name the encoder refuses.
fn fits(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if out.len() + c.len_utf8() > MAX_NAME_LEN {
            break;
        }
        out.push(c);
    }
    out
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
/// The three limits are the encoder's, not this app's: the field lattice is defined
/// against one rate, a stroke carries one channel or two and nothing else, and a stroke
/// shorter than [`encode::MIN_FRAMES`] has an opening the encoder does not model.
pub fn refusal(source: &Source) -> Option<String> {
    let pcm = match source {
        Source::Unreadable(why) => return Some(why.clone()),
        Source::Read(pcm) => pcm,
    };
    if pcm.rate != SOURCE_RATE {
        return Some(format!(
            "{} Hz — the field lattice is defined against {SOURCE_RATE} Hz and the \
             instrument's own resampler is not decoded, so resample the file first",
            pcm.rate
        ));
    }
    if pcm.channels != 1 && pcm.channels != 2 {
        return Some(format!(
            "{} channels — a stroke's terminator states one cell size, so it carries \
             one channel or two and nothing else",
            pcm.channels
        ));
    }
    if pcm.frames() < encode::MIN_FRAMES {
        return Some(format!(
            "{} frames — shorter than {} means an unresolved opening the encoder does \
             not model",
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
        .predictor(match draft.predict {
            true => encode::Predictor::Minimising,
            false => encode::Predictor::Plain,
        });
    let instrument = encode::instrument(&pcm.samples, &options).map_err(|e| e.to_string())?;
    instrument.to_bytes().map_err(|e| e.to_string())
}

/// Draw the panel. `true` once the operator has asked for the instrument to be made.
pub fn ui(ui: &mut egui::Ui, draft: &mut Draft, source: &Source) -> bool {
    ui.label(egui::RichText::new("This is a WAV, not a Nord file.").strong());
    ui.label(
        egui::RichText::new(
            "It can be encoded into a one-zone sample instrument. Encoding is \
             experimental: the file it writes is structurally sound and decodes back \
             exactly, but it is not byte-identical to the editor's own output. \
             Instruments encoded this way have been played on hardware.",
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
            ui.add(
                egui::TextEdit::singleline(&mut draft.name)
                    .desired_width(200.0)
                    .char_limit(MAX_NAME_LEN),
            );
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
            ui.add_space(120.0);
            ui.checkbox(&mut draft.predict, "Predict").on_hover_text(
                "the narrowest predictor order per cell: a smaller file, decoded \
                     back exactly either way",
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
                "one zone, root {}, up to {}",
                note::name(draft.root_key),
                note::name(draft.top_note)
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
    fn a_riff_wave_container_is_what_offers_the_panel() {
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

        // Two channels is a stereo stroke, not a refusal; three is neither.
        assert_eq!(refused(wav(SOURCE_RATE, 2, encode::MIN_FRAMES)), None);
        let wide = refused(wav(SOURCE_RATE, 3, encode::MIN_FRAMES)).expect("three channels");
        assert!(wide.contains("3 channels"), "{wide}");

        let short = refused(wav(SOURCE_RATE, 1, encode::MIN_FRAMES - 1)).expect("too short");
        assert!(short.contains(&encode::MIN_FRAMES.to_string()), "{short}");

        // Bytes that are not a readable WAV keep the reader's own complaint.
        let unreadable = refused(b"RIFF\0\0\0\0WAVE".to_vec()).expect("not readable");
        assert!(!unreadable.is_empty());
    }

    /// A stereo WAV encodes to a stereo stroke — both channels under one header, which
    /// the terminator's doubled cell is what states.
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
    fn a_refused_wav_is_not_encoded_anyway() {
        let source = Source::read(&wav(48_000, 1, encode::MIN_FRAMES));
        assert!(instrument(&Draft::new("x.wav"), &source).is_err());
    }

    #[test]
    fn a_long_filename_opens_the_panel_on_a_name_that_fits() {
        let draft = Draft::new("an extremely long marimba name.wav");
        assert_eq!(draft.name.len(), MAX_NAME_LEN);
        let source = Source::read(&wav(SOURCE_RATE, 1, encode::MIN_FRAMES));
        assert!(instrument(&draft, &source).is_ok());
    }
}
