//! New → Sample Editor project: some WAVs, the key each was recorded at, and the
//! `.nsmpproj` that comes out of them.
//!
//! The editor's own *Import Auto…* is what this imitates — one zone per file, ordered
//! by root key, key ranges derived from the roots. Everything the format needs beyond
//! the audio is the root key, and a filename is the only place a guess at one can come
//! from, so the dialog exists to let that guess be corrected before anything is made.
//!
//! ⚠️ A project holds **paths, not audio**. What lands in the list references the WAVs
//! by the names they were picked under, and the editor looks for them beside the
//! project file.

use eframe::egui;
use nord_format::formats::nsmp::codec::SOURCE_RATE;
use nord_format::formats::nsmpproj::{NewZone, Project, HIGHEST_NOTE, LOWEST_NOTE};
use nord_format::Entity;

use crate::document::note_picker;
use crate::log::Log;
use crate::note;
use crate::workspace::{Origin, Workspace};

/// Zones one project can hold: every key `Project::new` will lay a root on.
const MOST_ZONES: usize = (HIGHEST_NOTE - LOWEST_NOTE) as usize + 1;

/// One picked WAV, as the dialog shows it.
pub struct Take {
    /// The name the project will reference it by, resolved beside the project file.
    pub path: String,
    pub rate: u32,
    /// Frames as the project counts them — see [`at_source_rate`].
    pub frames: u64,
    pub root_key: u8,
    /// Why this file cannot be a zone, where it cannot.
    pub refusal: Option<String>,
}

/// The picked files, waiting on their root keys.
pub struct Draft {
    pub name: String,
    pub takes: Vec<Take>,
}

/// Frames at the 44 100 Hz basis a project counts in, whatever the file's own rate.
///
/// The editor stores positions against that rate for every file — a 0.1 s file stores
/// 4410 at 22 050 Hz and at 96 000 Hz alike.
pub fn at_source_rate(frames: u64, rate: u32) -> Option<u64> {
    if rate == 0 {
        return None;
    }
    let rate = u64::from(rate);
    frames
        .checked_mul(u64::from(SOURCE_RATE))
        .and_then(|scaled| scaled.checked_add(rate / 2))
        .map(|scaled| scaled / rate)
}

/// The key each file is taken to have been recorded at.
///
/// A note name on the end of a filename, where **every** file carries a distinct one —
/// that is the convention the corpus specimens are named by, and a run where one file
/// disagrees is a guess worth not making. Otherwise a chromatic run from middle C,
/// pulled down where it would not fit under the highest key a project maps.
pub fn default_roots(paths: &[String]) -> Vec<u8> {
    let named: Option<Vec<u8>> = paths.iter().map(|path| trailing_note(path)).collect();
    if let Some(named) = named {
        let mut sorted = named.clone();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.len() == named.len() {
            return named;
        }
    }
    let last = paths.len().saturating_sub(1) as u8;
    let start = 60.min(HIGHEST_NOTE.saturating_sub(last)).max(LOWEST_NOTE);
    (0..paths.len())
        .map(|i| start.saturating_add(i as u8).min(HIGHEST_NOTE))
        .collect()
}

/// A note name on the end of a file's stem: `Marimba-C3.wav` is C3.
///
/// The token has to start with a letter, so a trailing `1` is a take number rather
/// than MIDI note 1.
fn trailing_note(path: &str) -> Option<u8> {
    let stem = path.rsplit_once('.').map_or(path, |(stem, _)| stem);
    let token = stem.rsplit(['-', '_', ' ']).next()?;
    token
        .chars()
        .next()
        .filter(char::is_ascii_alphabetic)
        .and_then(|_| note::parse(token).ok())
        .filter(|note| (LOWEST_NOTE..=HIGHEST_NOTE).contains(note))
}

/// The name a project made from these files starts under.
fn project_name(paths: &[String]) -> String {
    let first = paths.first().map(String::as_str).unwrap_or_default();
    let stem = first.rsplit_once('.').map_or(first, |(stem, _)| stem);
    match stem.trim() {
        "" => "Untitled".to_string(),
        stem => stem.to_string(),
    }
}

impl Draft {
    /// What was picked, read for what a project needs to know about it.
    ///
    /// Nothing is refused whole here: a file that will not read is still listed, with
    /// the reason beside it, because the operator picked it on purpose.
    pub fn plan(files: Vec<(String, Vec<u8>)>) -> Option<Draft> {
        if files.is_empty() {
            return None;
        }
        let paths: Vec<String> = files.iter().map(|(name, _)| name.clone()).collect();
        let roots = default_roots(&paths);
        let takes = files
            .iter()
            .zip(roots)
            .map(|((path, bytes), root_key)| {
                let (rate, frames, refusal) = match nord_format::wav::read_pcm16(bytes) {
                    Ok(wav) => match at_source_rate(wav.frames() as u64, wav.rate) {
                        Some(frames) if frames > 0 => (wav.rate, frames, None),
                        _ => (wav.rate, 0, Some("it holds no audio".to_string())),
                    },
                    Err(e) => (0, 0, Some(e.to_string())),
                };
                Take {
                    path: path.clone(),
                    rate,
                    frames,
                    root_key,
                    refusal,
                }
            })
            .collect();
        Some(Draft {
            name: project_name(&paths),
            takes,
        })
    }

    /// Why this draft cannot be made into a project yet, in the operator's words.
    pub fn refusal(&self) -> Option<String> {
        if let Some(bad) = self.takes.iter().find(|take| take.refusal.is_some()) {
            return Some(format!(
                "{}: {}",
                bad.path,
                bad.refusal.clone().unwrap_or_default()
            ));
        }
        if self.takes.len() > MOST_ZONES {
            return Some(format!(
                "{} files — a project maps {MOST_ZONES} keys, so it holds at most that \
                 many zones",
                self.takes.len()
            ));
        }
        if let Some(take) = self
            .takes
            .iter()
            .find(|take| !(LOWEST_NOTE..=HIGHEST_NOTE).contains(&take.root_key))
        {
            return Some(format!(
                "{} is set to {} — a project maps {} to {}",
                take.path,
                note::name(take.root_key),
                note::name(LOWEST_NOTE),
                note::name(HIGHEST_NOTE),
            ));
        }
        let mut roots: Vec<u8> = self.takes.iter().map(|take| take.root_key).collect();
        roots.sort_unstable();
        roots
            .windows(2)
            .find(|pair| pair[0] == pair[1])
            .map(|pair| {
                format!(
                    "two files are set to {} — each zone needs a key of its own",
                    note::name(pair[0])
                )
            })
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        let zones: Vec<NewZone> = self
            .takes
            .iter()
            .map(|take| NewZone {
                path: take.path.clone(),
                sample_rate: take.rate,
                frames: take.frames,
                root_key: take.root_key,
            })
            .collect();
        let project = Project::new(&self.name, &zones, now()).map_err(|e| e.to_string())?;
        nord_format::to_bytes(&Entity::SampleProject(project)).map_err(|e| e.to_string())
    }
}

/// Unix seconds, for the `m_modifyDate` every block in a project carries.
#[cfg(not(target_arch = "wasm32"))]
fn now() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as u32)
}

/// ⚠️ `SystemTime::now` panics in a wasm module; the page's own clock is the only one.
#[cfg(target_arch = "wasm32")]
fn now() -> u32 {
    (js_sys::Date::now() / 1000.0) as u32
}

/// The dialog between picking WAVs and having a project, if one is waiting.
///
/// Returns the new asset once it is made, for the caller to open a tab on.
pub fn dialog(ctx: &egui::Context, workspace: &mut Workspace, log: &mut Log) -> Option<u64> {
    let mut make = false;
    let mut cancel = false;
    let draft = workspace.draft_mut()?;
    egui::Modal::new(egui::Id::new("new_project")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.heading("New Sample Editor project");
        ui.label(
            egui::RichText::new(
                "One zone per file, ordered by root key. The project stores the file names \
                 and the editor looks for them beside it, so keep them together.",
            )
            .small()
            .weak(),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(240.0));
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                for (i, take) in draft.takes.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [260.0, ui.spacing().interact_size.y],
                            egui::Label::new(&take.path)
                                .halign(egui::Align::LEFT)
                                .truncate(),
                        );
                        ui.label("root key");
                        if let Some(note) = note_picker(ui, ("draft_root", i), take.root_key) {
                            take.root_key = note;
                        }
                        match &take.refusal {
                            Some(why) => {
                                ui.label(
                                    egui::RichText::new(why)
                                        .small()
                                        .color(crate::app::bad(ui.visuals())),
                                );
                            }
                            None => {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} Hz, {:.2} s",
                                        take.rate,
                                        take.frames as f64 / f64::from(SOURCE_RATE)
                                    ))
                                    .small()
                                    .weak(),
                                );
                            }
                        }
                    });
                }
            });
        let refusal = draft.refusal();
        if let Some(why) = &refusal {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(why).color(crate::app::bad(ui.visuals())));
        }
        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            cancel = ui.button("Cancel").clicked();
            make = ui
                .add_enabled(
                    refusal.is_none(),
                    egui::Button::new(egui::RichText::new("Create").strong()),
                )
                .clicked();
        });
    });

    if cancel {
        workspace.take_draft();
        return None;
    }
    if !make {
        return None;
    }
    let draft = workspace.take_draft()?;
    match draft.bytes() {
        Ok(bytes) => Some(workspace.ingest(
            format!("{}.{}", draft.name, nord_format::formats::nsmpproj::FORMAT),
            Origin::Fresh,
            bytes,
            log,
        )),
        Err(why) => {
            log.error(format!("new project: {why}"));
            log.trouble("Could not make a project out of those files.");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_counted_at_the_source_rate() {
        assert_eq!(at_source_rate(44_100, 44_100), Some(44_100));
        assert_eq!(at_source_rate(22_050, 22_050), Some(44_100));
        assert_eq!(at_source_rate(96_000, 96_000), Some(44_100));
        // The 0.1 s case the format module states outright.
        assert_eq!(at_source_rate(2_205, 22_050), Some(4_410));
        assert_eq!(at_source_rate(9_600, 96_000), Some(4_410));
        assert_eq!(
            at_source_rate(1, 48_000),
            Some(1),
            "rounded to nearest, the way `nord sample project new` counts"
        );
        assert_eq!(at_source_rate(1, 0), None, "a rateless file has no basis");
        assert_eq!(at_source_rate(u64::MAX, 44_100), None, "no wrapping");
    }

    #[test]
    fn root_keys_are_read_off_the_names_or_counted_from_middle_c() {
        let named = ["Marimba-C3.wav", "Marimba-C4.wav", "Marimba_F#4.wav"].map(String::from);
        assert_eq!(default_roots(&named), vec![48, 60, 66]);

        // One file that names no key, and the whole run is a guess again.
        let mixed = ["Marimba-C3.wav", "Marimba-take2.wav"].map(String::from);
        assert_eq!(default_roots(&mixed), vec![60, 61]);

        // Two files naming one key would make a project that cannot be built.
        let clashing = ["a-C3.wav", "b-C3.wav"].map(String::from);
        assert_eq!(default_roots(&clashing), vec![60, 61]);

        // A trailing number is a take, not MIDI note 1.
        assert_eq!(trailing_note("hit-1.wav"), None);
        assert_eq!(trailing_note("hit-Bb2.wav"), Some(46));
    }

    #[test]
    fn the_default_run_fits_under_the_highest_key_a_project_maps() {
        let many: Vec<String> = (0..MOST_ZONES).map(|i| format!("{i}.wav")).collect();
        let roots = default_roots(&many);
        assert_eq!(roots.first(), Some(&LOWEST_NOTE));
        assert_eq!(roots.last(), Some(&HIGHEST_NOTE));
        let mut unique = roots.clone();
        unique.dedup();
        assert_eq!(unique.len(), roots.len(), "one key each");
    }

    fn wav(rate: u32, frames: usize) -> Vec<u8> {
        nord_format::wav::mono_pcm16(&vec![0i16; frames], rate).unwrap()
    }

    #[test]
    fn a_draft_becomes_a_project_over_the_picked_files() {
        let draft = Draft::plan(vec![
            ("Low-C3.wav".into(), wav(44_100, 4_410)),
            ("High-C5.wav".into(), wav(22_050, 2_205)),
        ])
        .expect("two files were picked");
        assert_eq!(draft.name, "Low-C3");
        assert_eq!(
            draft.takes.iter().map(|t| t.root_key).collect::<Vec<_>>(),
            vec![48, 72]
        );
        // Both are a tenth of a second, so both count 4410 frames.
        assert_eq!(draft.takes[0].frames, 4_410);
        assert_eq!(draft.takes[1].frames, 4_410);
        assert_eq!(draft.takes[1].rate, 22_050, "the file's own rate is kept");
        assert!(draft.refusal().is_none());

        let bytes = draft.bytes().expect("a project");
        let entity = nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap();
        let Entity::SampleProject(project) = &entity else {
            panic!("a Sample Editor project");
        };
        assert_eq!(project.name().unwrap(), "Low-C3");
        let paths: Vec<String> = project
            .audio_files()
            .unwrap()
            .into_iter()
            .map(|file| file.path)
            .collect();
        assert_eq!(paths, ["Low-C3.wav", "High-C5.wav"]);
        let roots: Vec<u8> = project
            .zones()
            .unwrap()
            .iter()
            .map(|zone| zone.root_key)
            .collect();
        assert_eq!(roots, [72, 48], "zones are stored high to low");
        assert_eq!(nord_format::to_bytes(&entity).unwrap(), bytes);
    }

    #[test]
    fn a_cancelled_pick_raises_no_dialog() {
        assert!(Draft::plan(Vec::new()).is_none());
    }

    #[test]
    fn a_draft_says_why_it_cannot_be_made() {
        let unreadable = Draft::plan(vec![("notes.txt".into(), b"not a wav".to_vec())]).unwrap();
        let why = unreadable.refusal().expect("it will not read");
        assert!(why.starts_with("notes.txt: "), "{why}");

        let mut clashing = Draft::plan(vec![
            ("a.wav".into(), wav(44_100, 4_410)),
            ("b.wav".into(), wav(44_100, 4_410)),
        ])
        .unwrap();
        assert!(clashing.refusal().is_none());
        clashing.takes[1].root_key = clashing.takes[0].root_key;
        let why = clashing.refusal().expect("two zones on one key");
        assert!(why.contains("C4"), "{why}");
        assert!(clashing.bytes().is_err());
    }
}
