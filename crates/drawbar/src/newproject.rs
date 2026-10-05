//! New → a Sample Editor project, a sample instrument, or a piano library: some WAVs,
//! the key each was recorded at, and the `.nsmpproj`, `.nsmp`, or `.npno` made from them.
//!
//! The first two imitate the Sample Editor's *Import Auto…*: one zone per file, ordered
//! by root key, with key ranges derived from the roots. Beyond the audio, either format
//! needs only the root key, and the filename is the only source of a guess at one. The
//! dialog lets the user correct that guess before anything is made.
//!
//! A piano library needs more per file (a bank and a velocity layer as well as a root),
//! and encoding it takes long enough that the build runs in the background.
//!
//! ⚠️ A project holds **paths, not audio**. A new one goes in a folder of its own with a
//! copy of each WAV it plays, each named by a bare leaf, so the project builds where it
//! lands and the Sample Editor finds its WAVs beside it. An instrument and a library hold
//! the audio itself, so the encoder's limits on what a WAV may be apply to them.

use eframe::egui;
use nord_format::formats::npno::encode::{
    build, parse_stroke_name, resample, Clash, Donor, Kind, LayerTag, Options, Recording, Rules,
    Stem, HIGHEST_PLAYED_LAYER,
};
use nord_format::formats::npno::{Bank, Library};
use nord_format::formats::nsmp::codec::Layout;
use nord_format::formats::nsmp::zone::derive_top_notes;
use nord_format::formats::nsmp::{encode, MAX_NAME_LEN};
use nord_format::formats::nsmpproj::{
    project_frames, NewZone, Project, HIGHEST_NOTE, LOWEST_NOTE, PROJECT_RATE,
};
use nord_format::note;
use nord_format::wav::Pcm16;
use nord_format::Entity;

use crate::document::controls::fits;
use crate::document::encode::{encodable, read, Source};
use crate::document::note_picker;
use crate::folders::Folders;
use crate::log::Log;
use crate::store::{names, LibPath};
use crate::work::{self, Job, Progress};
use crate::workspace::{Origin, Workspace};

/// The most zones one draft can hold: one per key the dialog can set a root on.
const MOST_ZONES: usize = (HIGHEST_NOTE - LOWEST_NOTE) as usize + 1;

/// The root assumed for a file whose name gives none.
const MIDDLE_C: u8 = 60;

/// What a pick of WAVs is turned into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Making {
    /// A `.nsmpproj`: the file names, and where each sits on the keyboard.
    Project,
    /// A `.nsmp`: the audio itself, one zone per file, in the generation that has been
    /// played on hardware. The document panel for a WAV is where a generation is chosen;
    /// this makes the one that plays.
    Instrument,
    /// A `.npno`: one stroke per file, resampled to the rate the piano section plays at
    /// and encoded into the library.
    Piano,
}

impl Making {
    /// Everything a pick of WAVs makes.
    pub const FROM_WAVS: [Making; 3] = [Making::Project, Making::Instrument, Making::Piano];

    /// Whether this makes a file an instrument holds, which decides the side of the New
    /// menu's separator it goes on.
    ///
    /// A project is the Sample Editor's save file. It builds an instrument, but no
    /// instrument has a folder for the project itself.
    pub fn instrument_file(self) -> bool {
        match self {
            Making::Instrument | Making::Piano => true,
            Making::Project => false,
        }
    }

    /// What it makes, as the dialog, the file picker, and the log name it.
    pub fn label(self) -> &'static str {
        match self {
            Making::Project => "Sample Editor project",
            Making::Instrument => "sample instrument",
            Making::Piano => "piano library",
        }
    }

    /// The New menu item's label and hover text.
    pub fn item(self) -> (&'static str, &'static str) {
        match self {
            Making::Project => (
                "Sample Editor project…",
                "pick the WAVs it plays; the project and a copy of each WAV go in a new \
                 folder named after it",
            ),
            Making::Instrument => (
                "Sample instrument…",
                "pick the WAVs it plays; the audio is encoded into the instrument, so \
                 the files are not needed afterward",
            ),
            Making::Piano => (
                "Piano library…",
                "pick one WAV per stroke; the audio is encoded into the library, so the \
                 files are not needed afterward",
            ),
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Making::Project => nord_format::formats::nsmpproj::FORMAT,
            Making::Instrument => Layout::V2.extension(),
            Making::Piano => nord_format::formats::npno::FORMAT,
        }
    }

    fn caption(self) -> &'static str {
        match self {
            Making::Project => {
                "One zone per file, ordered by root key. The project and a copy of each \
                 WAV go in a new folder named after the project."
            }
            Making::Instrument => {
                "One zone per file, ordered by root key. The audio is encoded into the \
                 instrument, so the WAVs are not needed afterward."
            }
            Making::Piano => {
                "One stroke per WAV. A name like 060-b0-l00.wav sets the root, bank and \
                 layer; set them here otherwise. Kind, gain and the damper limit are \
                 edited in the document afterward."
            }
        }
    }
}

/// One picked WAV, as the dialog shows it.
pub struct Take {
    /// The name it was picked under.
    pub path: String,
    /// The file itself, which a new project is filed beside. `None` for what carries the
    /// audio instead.
    file: Option<Vec<u8>>,
    /// The decoded file, or the reader's error.
    pub source: Source,
    /// Frames as a project counts them (see [`project_frames`]). Zero if the file did not
    /// read or holds no audio.
    pub frames: u64,
    pub root_key: u8,
    /// Which of a piano library's three banks the stroke belongs to. Unused for a zone.
    pub bank: Bank,
    /// Where the stroke sits among its root and bank's layers. Unused for a zone.
    pub layer: LayerTag,
}

impl Take {
    /// A picked file, keyed at `root` as a zone. A piano stroke takes its key, bank and
    /// layer from [`stroke_defaults`] instead.
    fn new(making: Making, path: String, bytes: Vec<u8>, root: u8) -> Take {
        let (root_key, bank, layer) = match making {
            Making::Piano => stroke_defaults(&path),
            Making::Project | Making::Instrument => (root, Bank::Attack, LayerTag::Index(0)),
        };
        let source = read(&bytes);
        let frames = match &source {
            Ok(pcm) => project_frames(pcm.frames() as u64, pcm.rate).unwrap_or_default(),
            Err(_) => 0,
        };
        let file = (making == Making::Project).then_some(bytes);
        Take {
            path,
            file,
            source,
            frames,
            root_key,
            bank,
            layer,
        }
    }

    fn pcm(&self) -> Option<&Pcm16> {
        self.source.as_ref().ok()
    }

    /// Why this file cannot be part of what is being made.
    ///
    /// A project references audio without reading it, so any file with audio will do. An
    /// instrument carries the audio, so the encoder's limits apply. A library accepts any
    /// rate because it resamples, and reports its other limits when it is built.
    pub fn refusal(&self, making: Making) -> Option<String> {
        match making {
            Making::Instrument => encodable(&self.source).err(),
            Making::Project => match &self.source {
                Err(why) => Some(why.clone()),
                Ok(_) => (self.frames == 0).then(|| "it holds no audio".to_string()),
            },
            Making::Piano => self.source.as_ref().err().cloned(),
        }
    }
}

/// The picked files, waiting on their root keys.
pub struct Draft {
    pub making: Making,
    pub name: String,
    pub takes: Vec<Take>,
    /// The piano document a build copies its playback fields from, if one is chosen.
    pub template: Option<u64>,
    /// The build in flight, once Create has been pressed.
    job: Option<Job<Result<Built, String>>>,
    /// Why the last build failed, kept with the takes so they can be fixed.
    refused: Option<String>,
}

/// The key each file is assumed to have been recorded at.
///
/// The note name at the end of each filename, if **every** file carries a distinct one.
/// If any file lacks one or two share one, no name is trusted. Otherwise a chromatic run
/// from middle C, moved down if it would not fit under the highest key a project maps.
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
    // ⚠️ Counted in usize: a pick can hold more files than there are keys, and the run
    // must still start from one end of the keyboard.
    let after_first = paths.len().min(MOST_ZONES).saturating_sub(1);
    let start = u8::try_from(usize::from(HIGHEST_NOTE).saturating_sub(after_first))
        .unwrap_or(LOWEST_NOTE)
        .clamp(LOWEST_NOTE, MIDDLE_C);
    (start..=HIGHEST_NOTE)
        .chain(std::iter::repeat(HIGHEST_NOTE))
        .take(paths.len())
        .collect()
}

/// The note name at the end of a file's stem: `Marimba-C3.wav` is C3.
///
/// The token must start with a letter, so a trailing `1` is a take number, not MIDI
/// note 1.
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

/// The stroke a WAV's name states: `060-b0-l00`, as `nord piano build` reads it, and
/// `<stem>-060-b0-l00` as [`crate::workspace::stroke_wav_name`] writes it.
fn stroke_name(path: &str) -> Option<(u8, Bank, LayerTag)> {
    let stem = path.rsplit_once('.').map_or(path, |(stem, _)| stem);
    parse_stroke_name(stem, Stem::Any)
}

/// Whether a dropped file goes to an open draft instead of opening as a document. Only
/// the extension is checked; a file that will not read is listed with the reader's error
/// beside it.
pub fn is_wav_name(name: &str) -> bool {
    crate::browser::Kind::of_name(name) == crate::browser::Kind::Wav
}

/// The stroke a picked WAV is assumed to hold: the one its name states, or else a root
/// from the end of the name and the loudest layer of the attack bank.
fn stroke_defaults(path: &str) -> (u8, Bank, LayerTag) {
    stroke_name(path).unwrap_or((
        trailing_note(path).unwrap_or(MIDDLE_C),
        Bank::Attack,
        LayerTag::Index(0),
    ))
}

/// The starting name for a draft of these files: the first file's stem, truncated to fit
/// an instrument's name field.
fn draft_name(making: Making, paths: &[String]) -> String {
    let first = paths.first().map(String::as_str).unwrap_or_default();
    let stem = first.rsplit_once('.').map_or(first, |(stem, _)| stem);
    // Drop a stroke suffix: `Grand-060-b0-l00` starts as `Grand`.
    let stem = match making == Making::Piano && stroke_name(stem).is_some() {
        true => stem.rsplitn(4, '-').nth(3).unwrap_or_default(),
        false => stem,
    };
    let stem = match stem.trim() {
        "" => "Untitled",
        stem => stem,
    };
    let mut name = stem.to_string();
    match making {
        Making::Project | Making::Piano => {}
        Making::Instrument => fits(&mut name, MAX_NAME_LEN),
    }
    name
}

impl Draft {
    /// Read the picked files for what the dialog needs to know about them.
    ///
    /// Nothing is dropped here: a file that will not read is still listed, with the
    /// reason beside it, because the user picked it.
    pub fn plan(making: Making, files: Vec<(String, Vec<u8>)>) -> Option<Draft> {
        if files.is_empty() {
            return None;
        }
        let paths: Vec<String> = files.iter().map(|(name, _)| name.clone()).collect();
        let takes = files
            .into_iter()
            .zip(default_roots(&paths))
            .map(|((path, bytes), root)| Take::new(making, path, bytes, root))
            .collect();
        Some(Draft {
            making,
            name: draft_name(making, &paths),
            takes,
            template: None,
            job: None,
            refused: None,
        })
    }

    /// Add more files, as a drop onto the open dialog does.
    pub fn add(&mut self, files: Vec<(String, Vec<u8>)>) {
        for (path, bytes) in files {
            let root = trailing_note(&path).unwrap_or_else(|| self.free_key());
            let take = Take::new(self.making, path, bytes, root);
            self.takes.push(take);
        }
    }

    /// The lowest key a project maps that no take uses. A dropped file whose name gives
    /// no key goes there, since each zone needs its own key.
    fn free_key(&self) -> u8 {
        (LOWEST_NOTE..=HIGHEST_NOTE)
            .find(|key| self.takes.iter().all(|take| take.root_key != *key))
            .unwrap_or(HIGHEST_NOTE)
    }

    /// Why this draft cannot be made yet, in words for the user.
    pub fn refusal(&self) -> Option<String> {
        if let Some((take, why)) = self
            .takes
            .iter()
            .find_map(|take| Some((take, take.refusal(self.making)?)))
        {
            return Some(format!("{}: {why}", take.path));
        }
        // ⚠️ The library encoder checks every other rule about its strokes when it
        // builds, and reports it with the takes still here to be fixed.
        if self.making == Making::Piano {
            return layer_values(&self.takes).err();
        }
        if self.making == Making::Instrument && self.name.len() > MAX_NAME_LEN {
            return Some(format!(
                "the name is {} bytes, but an instrument's name field holds \
                 {MAX_NAME_LEN}",
                self.name.len()
            ));
        }
        if self.takes.len() > MOST_ZONES {
            return Some(format!(
                "{} files, but each file needs its own zone and a project maps only \
                 {MOST_ZONES} keys",
                self.takes.len()
            ));
        }
        if let Some(take) = self
            .takes
            .iter()
            .find(|take| !(LOWEST_NOTE..=HIGHEST_NOTE).contains(&take.root_key))
        {
            return Some(format!(
                "{} is set to {}, but the keys run from {} to {}",
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
                    "two files are set to {}, but each zone needs its own key",
                    note::name(pair[0])
                )
            })
    }

    /// The file the picked WAVs make, built within the current frame.
    ///
    /// ⚠️ Only a project or an instrument. Building a library takes longer than a frame,
    /// so the dialog starts that build with [`Draft::begin`] instead, and this refuses a
    /// library.
    fn bytes(&self) -> Result<Vec<u8>, String> {
        match self.making {
            Making::Project => self.project(),
            Making::Instrument => self.instrument(),
            Making::Piano => Err("a piano library is built in the background".to_string()),
        }
    }

    /// Everything a build needs, owned, so it can run apart from the dialog.
    fn coding(&self, donor: Option<Library<'static>>) -> Result<Coding, String> {
        let values = layer_values(&self.takes)?;
        let mut takes = Vec::with_capacity(self.takes.len());
        for (take, layer) in self.takes.iter().zip(values) {
            let pcm = take
                .pcm()
                .ok_or_else(|| format!("{}: it did not read as a WAV", take.path))?;
            takes.push(Recorded {
                path: take.path.clone(),
                samples: pcm.samples.clone(),
                channels: pcm.channels,
                rate: pcm.rate,
                root: take.root_key,
                bank: take.bank,
                layer,
            });
        }
        Ok(Coding {
            name: self.name.clone(),
            donor,
            takes,
        })
    }

    /// Start building the library, copying from `donor` if a template was chosen.
    fn begin(
        &mut self,
        ctx: &egui::Context,
        donor: Option<Library<'static>>,
    ) -> Result<(), String> {
        let coding = self.coding(donor)?;
        self.refused = None;
        self.job = Some(work::run(ctx, move |progress| coding.run(progress)));
        Ok(())
    }

    /// The build's result once it is ready, returned once. A failure is also kept for
    /// the dialog to show.
    fn settle(&mut self) -> Option<Result<Built, String>> {
        let answer = match self.job.as_ref()?.poll() {
            work::Answer::Running => return None,
            work::Answer::Answered(answer) => answer,
            work::Answer::Died => Err("building the library stopped without a result".to_string()),
        };
        self.job = None;
        if let Err(why) = &answer {
            self.refused = Some(why.clone());
        }
        Some(answer)
    }

    /// Make what the draft describes, as a new asset: an instrument to be filed like any
    /// new asset, or a project filed at once in a new folder in the library's root, named
    /// after it and numbered past what is there, with its WAVs beside it under bare leaf
    /// names, numbered where two picks collide.
    pub fn make(
        &self,
        workspace: &mut Workspace,
        folders: &mut Folders,
        log: &mut Log,
    ) -> Result<u64, String> {
        let bytes = self.bytes()?;
        let name = format!("{}.{}", self.name, self.making.extension());
        if self.making != Making::Project {
            return Ok(workspace.ingest(name, Origin::Fresh, bytes, log));
        }
        let mut copies = Vec::new();
        let mut filed = std::collections::BTreeSet::new();
        for (take, leaf) in self.takes.iter().zip(leaves(&self.takes)) {
            if !filed.insert(names::key(&leaf)) {
                continue;
            }
            let bytes = take
                .file
                .clone()
                .ok_or_else(|| format!("{}: its bytes were not kept", take.path))?;
            copies.push((leaf, bytes));
        }
        let root = LibPath::root();
        let folder = folders.free(&root, &names::portable(&self.name), workspace);
        folders
            .named_or_made(&root, &folder, workspace)
            .ok_or_else(|| format!("“{folder}” is taken"))?;
        let dir = root.join(&folder);
        let name = names::portable(&name);
        let made = workspace.ingest(name.clone(), Origin::Fresh, bytes, log);
        workspace.place(made, dir.join(&name));
        for (leaf, bytes) in copies {
            let wav = workspace.ingest(leaf.clone(), Origin::Fresh, bytes, log);
            workspace.place(wav, dir.join(&leaf));
        }
        Ok(made)
    }

    fn project(&self) -> Result<Vec<u8>, String> {
        let zones: Vec<NewZone> = self
            .takes
            .iter()
            .zip(leaves(&self.takes))
            .map(|(take, path)| NewZone {
                path,
                sample_rate: take.pcm().map_or(0, |pcm| pcm.rate),
                frames: take.frames,
                root_key: take.root_key,
            })
            .collect();
        let modified = crate::work::unix_seconds().unwrap_or(0);
        let project = Project::new(&self.name, &zones, modified).map_err(|e| e.to_string())?;
        nord_format::to_bytes(&Entity::SampleProject(project)).map_err(|e| e.to_string())
    }

    /// One `stk` per file, highest root first, each zone reaching up to where
    /// [`derive_top_notes`] puts it. This is the layout `Project::new` writes, with the
    /// audio encoded instead of referenced.
    fn instrument(&self) -> Result<Vec<u8>, String> {
        let mut order: Vec<&Take> = self.takes.iter().collect();
        order.sort_by_key(|take| std::cmp::Reverse(take.root_key));
        let roots: Vec<u8> = order.iter().map(|take| take.root_key).collect();
        let tops = derive_top_notes(&roots);
        let mut audio = Vec::with_capacity(order.len());
        for take in &order {
            audio.push(
                take.pcm()
                    .ok_or_else(|| format!("{}: it did not read as a WAV", take.path))?,
            );
        }
        let zones: Vec<encode::NewZone> = order
            .iter()
            .zip(&audio)
            .zip(&tops)
            .enumerate()
            .map(|(i, ((take, pcm), top_note))| encode::NewZone {
                source: &pcm.samples,
                channels: pcm.channels,
                root_key: take.root_key,
                top_note: *top_note,
                global_id: i as u32 + 1,
                loops: None,
                secondary_start: encode::default_secondary_start(pcm.frames(), None),
                shift: None,
                gain: 1.0,
                loop_decay: encode::DEFAULT_LOOP_DECAY,
            })
            .collect();
        let instrument = encode::multi_zone(
            encode::Instrument {
                name: &self.name,
                map_gain: 1.0,
                predictor: encode::Predictor::Minimizing,
                layout: Layout::V2,
                preset: encode::Preset::default(),
            },
            &zones,
        )
        .map_err(|e| e.to_string())?;
        instrument.to_bytes().map_err(|e| e.to_string())
    }
}

/// The leaf each take's WAV is filed under beside a new project: the leaf it was picked
/// under, made portable, and numbered past an earlier take's under the same
/// [`names::key`], unless the two hold the same bytes and so share one file.
fn leaves(takes: &[Take]) -> Vec<String> {
    let mut filed: Vec<(String, String, Option<&[u8]>)> = Vec::new();
    takes
        .iter()
        .map(|take| {
            let picked = take.path.rsplit(['/', '\\']).next().unwrap_or_default();
            let wanted = names::portable(picked);
            let (key, bytes) = (names::key(&wanted), take.file.as_deref());
            if let Some((_, leaf, _)) = filed
                .iter()
                .find(|(held, _, same)| *held == key && *same == bytes)
            {
                return leaf.clone();
            }
            let leaf = names::free(&wanted, |taken| {
                filed.iter().any(|(_, leaf, _)| names::key(leaf) == taken)
            });
            filed.push((key, leaf.clone(), bytes));
            leaf
        })
        .collect()
}

/// One take as a build reads it: the file's audio, and the stroke it becomes.
struct Recorded {
    path: String,
    samples: Vec<i16>,
    channels: u16,
    rate: u32,
    root: u8,
    bank: Bank,
    layer: u8,
}

/// A piano library build, detached from the dialog that set it up.
struct Coding {
    name: String,
    donor: Option<Library<'static>>,
    takes: Vec<Recorded>,
}

/// A built library, and what the build reports about it.
#[derive(Debug)]
struct Built {
    bytes: Vec<u8>,
    strokes: usize,
    roots: usize,
    /// Samples that saturated at int16 while resampling.
    clipped: usize,
}

impl Coding {
    fn run(self, progress: &Progress) -> Result<Built, String> {
        let count = self.takes.len();
        let mut recordings = Vec::with_capacity(count);
        let mut clipped = 0usize;
        for (done, take) in self.takes.iter().enumerate() {
            progress.say(format!("resampling {} of {count}", done + 1));
            let audio = resample(&take.samples, usize::from(take.channels), take.rate)
                .map_err(|e| format!("{}: {e}", take.path))?;
            clipped += audio.clipped;
            recordings.push(Recording {
                root: take.root,
                bank: take.bank,
                layer: take.layer,
                channels: audio.channels,
            });
        }

        progress.say(format!("coding {count} strokes…"));
        let donor = match &self.donor {
            Some(template) => Donor::Template(template),
            None => Donor::Rules(Rules::new(Kind::Grand)),
        };
        let library =
            build(&donor, &Options::new(&self.name), &recordings).map_err(|e| e.to_string())?;
        let (strokes, roots) = (library.strokes().len(), library.roots().len());
        let piano = library.to_piano().map_err(|e| e.to_string())?;
        Ok(Built {
            bytes: nord_format::to_bytes(&Entity::Piano(piano)).map_err(|e| e.to_string())?,
            strokes,
            roots,
            clipped,
        })
    }
}

/// The layer value of each take's stroke, in list order, as
/// [`nord_format::formats::npno::encode::layer_values`] computes it.
fn layer_values(takes: &[Take]) -> Result<Vec<u8>, String> {
    let named: Vec<(u8, Bank, LayerTag)> = takes
        .iter()
        .map(|take| (take.root_key, take.bank, take.layer))
        .collect();
    nord_format::formats::npno::encode::layer_values(&named).map_err(|clash| {
        let what = format!("root {} {}", note::name(clash.root), clash.bank.name());
        match clash.how {
            Clash::BothForms => format!(
                "{what} names some of its layers by index and some by value; one root's \
                 bank names them one way"
            ),
            Clash::Twice => format!("{what} names one of its layers twice"),
        }
    })
}

/// The dialog between picking WAVs and having what they make, if a draft is waiting.
///
/// Returns the new asset once it is made, for the caller to open a tab on.
pub fn dialog(
    ctx: &egui::Context,
    workspace: &mut Workspace,
    folders: &mut Folders,
    log: &mut Log,
) -> Option<u64> {
    if let Some(made) = answered(workspace, log) {
        return Some(made);
    }
    let making = workspace.draft_mut()?.making;
    let templates = match making {
        Making::Piano => templates(workspace),
        Making::Project | Making::Instrument => Vec::new(),
    };

    let mut make = false;
    let mut cancel = false;
    let draft = workspace.draft_mut()?;
    let coding = draft.job.is_some();
    let progress = draft.job.as_ref().map(Job::progress).unwrap_or_default();
    egui::Modal::new(egui::Id::new("new_project")).show(ctx, |ui| {
        ui.set_width(match making {
            Making::Piano => 660.0,
            Making::Project | Making::Instrument => 520.0,
        });
        ui.heading(format!("New {}", making.label()));
        ui.label(egui::RichText::new(making.caption()).small().weak());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(240.0));
        });
        ui.add_space(4.0);
        let column = match making {
            Making::Piano => 170.0,
            Making::Project | Making::Instrument => 260.0,
        };
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                for (i, take) in draft.takes.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [column, ui.spacing().interact_size.y],
                            egui::Label::new(&take.path)
                                .halign(egui::Align::LEFT)
                                .truncate(),
                        );
                        ui.label(match making {
                            Making::Piano => "root",
                            Making::Project | Making::Instrument => "root key",
                        });
                        if let Some(note) = note_picker(ui, ("draft_root", i), take.root_key) {
                            take.root_key = note;
                        }
                        if making == Making::Piano {
                            stroke_controls(ui, i, take);
                        }
                        match (take.refusal(making), take.pcm()) {
                            (Some(why), _) => {
                                ui.label(
                                    egui::RichText::new(why)
                                        .small()
                                        .color(crate::app::bad(ui.visuals())),
                                );
                            }
                            (None, Some(pcm)) => {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} Hz, {:.2} s",
                                        pcm.rate,
                                        take.frames as f64 / PROJECT_RATE as f64
                                    ))
                                    .small()
                                    .weak(),
                                );
                            }
                            (None, None) => {}
                        }
                    });
                }
            });
        if making == Making::Piano {
            ui.add_space(4.0);
            egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
                template_picker(ui, draft, &templates);
            });
        }
        let refusal = draft.refusal();
        if let Some(why) = refusal.as_deref().or(draft.refused.as_deref()) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(why).color(crate::app::bad(ui.visuals())));
        }
        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            cancel = ui.button("Cancel").clicked();
            make = ui
                .add_enabled(
                    refusal.is_none() && !coding,
                    egui::Button::new(egui::RichText::new("Create").strong()),
                )
                .clicked();
            if coding {
                ui.spinner();
                ui.label(egui::RichText::new(&progress).small().weak());
            }
        });
    });

    if cancel {
        workspace.take_draft();
        return None;
    }
    if !make {
        return None;
    }
    if making == Making::Piano {
        start(ctx, workspace, log);
        // ⚠️ wasm runs the build on the spot, so the result is already here.
        return answered(workspace, log);
    }
    let draft = workspace.take_draft()?;
    match draft.make(workspace, folders, log) {
        Ok(made) => Some(made),
        Err(why) => {
            log.error(format!("new {}: {why}", making.label()));
            log.trouble(format!(
                "Could not make a {} out of those files.",
                making.label()
            ));
            None
        }
    }
}

/// The bank and layer controls a piano take has beyond a root key.
fn stroke_controls(ui: &mut egui::Ui, i: usize, take: &mut Take) {
    egui::ComboBox::from_id_salt(("draft_bank", i))
        .width(92.0)
        .selected_text(take.bank.name())
        .show_ui(ui, |ui| {
            for bank in Bank::ALL {
                ui.selectable_value(&mut take.bank, bank, bank.name());
            }
        });
    let (number, word) = match take.layer {
        LayerTag::Index(n) => (n, "index"),
        LayerTag::Value(n) => (n, "value"),
    };
    egui::ComboBox::from_id_salt(("draft_layer", i))
        .width(64.0)
        .selected_text(word)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut take.layer, LayerTag::Index(number), "index");
            ui.selectable_value(&mut take.layer, LayerTag::Value(number), "value");
        });
    let mut set = number;
    // The value scale is the format's: above HIGHEST_PLAYED_LAYER no velocity selects the
    // stroke, and `build` refuses it. An index ranks the takes sharing a root and bank,
    // and no root plays more layers than that either.
    if ui
        .add(egui::DragValue::new(&mut set).range(0..=HIGHEST_PLAYED_LAYER))
        .changed()
    {
        take.layer = match take.layer {
            LayerTag::Index(_) => LayerTag::Index(set),
            LayerTag::Value(_) => LayerTag::Value(set),
        };
    }
}

/// The template picker's label for no template, also shown when the list is empty.
const NO_TEMPLATE: &str = "none (default rules)";

fn template_picker(ui: &mut egui::Ui, draft: &mut Draft, templates: &[(u64, String)]) {
    ui.horizontal(|ui| {
        ui.label("Template");
        let shown = draft
            .template
            .and_then(|id| templates.iter().find(|(open, _)| *open == id))
            .map_or(NO_TEMPLATE, |(_, name)| name.as_str());
        egui::ComboBox::from_id_salt("draft_template")
            .width(300.0)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.template, None, NO_TEMPLATE);
                for (id, name) in templates {
                    ui.selectable_value(&mut draft.template, Some(*id), name);
                }
            });
    });
}

/// The piano documents on this computer, by name, for a build to donate from.
fn templates(workspace: &Workspace) -> Vec<(u64, String)> {
    let mut open: Vec<(u64, String)> = workspace
        .entities()
        .iter()
        .filter(|entity| {
            matches!(entity.entity.as_deref(), Some(Entity::Piano(_)))
                || matches!(entity.indexed(), Some(crate::ondisk::Index::Piano(_)))
        })
        .map(|entity| (entity.id, entity.name.clone()))
        .collect();
    open.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    open
}

/// A template's prefix and stroke records, read here because a build cannot borrow the
/// document's bytes.
fn skeleton(workspace: &Workspace, id: u64) -> Result<Library<'static>, String> {
    let entity = workspace
        .get(id)
        .ok_or_else(|| "the template is no longer open".to_string())?;
    if let Some(crate::ondisk::Index::Piano(index)) = entity.indexed() {
        return Ok(index.library().clone());
    }
    Library::borrow(&entity.bytes)
        .map(|library| library.without_audio())
        .map_err(|e| format!("{}: {e}", entity.name))
}

/// Start the build with the template chosen under Advanced, if any.
fn start(ctx: &egui::Context, workspace: &mut Workspace, log: &mut Log) {
    let chosen = workspace.draft_mut().and_then(|draft| draft.template);
    let donor = match chosen {
        Some(id) => skeleton(workspace, id).map(Some),
        None => Ok(None),
    };
    let Some(draft) = workspace.draft_mut() else {
        return;
    };
    if let Err(why) = donor.and_then(|donor| draft.begin(ctx, donor)) {
        draft.refused = Some(why.clone());
        log.error(format!("new piano library: {why}"));
    }
}

/// Take the build's result once it is ready, add the library, and log the outcome.
fn answered(workspace: &mut Workspace, log: &mut Log) -> Option<u64> {
    let draft = workspace.draft_mut()?;
    let built = match draft.settle()? {
        Ok(built) => built,
        Err(why) => {
            log.error(format!("new piano library: {why}"));
            return None;
        }
    };
    let name = format!("{}.{}", draft.name, Making::Piano.extension());
    workspace.take_draft();
    let id = workspace.ingest(name, Origin::Fresh, built.bytes, log);
    if built.clipped > 0 {
        log.warn(format!(
            "{} resampled sample(s) saturated at int16",
            built.clipped
        ));
    }
    log.say(format!(
        "Built {} stroke(s) over {} root(s).",
        built.strokes, built.roots
    ));
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_format::formats::npno::encode::SOFTEST_LAYER;
    use nord_format::formats::nsmp::codec::SOURCE_RATE;

    #[test]
    fn root_keys_are_read_off_the_names_or_counted_from_middle_c() {
        let named = ["Marimba-C3.wav", "Marimba-C4.wav", "Marimba_F#4.wav"].map(String::from);
        assert_eq!(default_roots(&named), vec![48, 60, 66]);

        // If one file names no key, the whole run is guessed.
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

    /// ⚠️ A pick this long is refused for its count, but the default run still climbs
    /// from the lowest key, one file per key while keys last.
    #[test]
    fn a_pick_longer_than_a_u8_counts_still_walks_up_from_the_lowest_key() {
        let many: Vec<String> = (0..MOST_ZONES + 200).map(|i| format!("{i}.wav")).collect();
        let roots = default_roots(&many);
        assert_eq!(roots.len(), many.len(), "a key per file");
        assert_eq!(roots.first(), Some(&LOWEST_NOTE));
        assert_eq!(roots[MOST_ZONES - 1], HIGHEST_NOTE);
        let mut laid = roots[..MOST_ZONES].to_vec();
        laid.dedup();
        assert_eq!(laid.len(), MOST_ZONES, "one key each, while there are keys");
    }

    fn wav(rate: u32, frames: usize) -> Vec<u8> {
        nord_format::wav::mono_pcm16(&vec![0i16; frames], rate).unwrap()
    }

    #[test]
    fn a_draft_becomes_a_project_over_the_picked_files() {
        let draft = Draft::plan(
            Making::Project,
            vec![
                ("Low-C3.wav".into(), wav(44_100, 4_410)),
                ("High-C5.wav".into(), wav(22_050, 2_205)),
            ],
        )
        .expect("two files were picked");
        assert_eq!(draft.name, "Low-C3");
        assert_eq!(
            draft.takes.iter().map(|t| t.root_key).collect::<Vec<_>>(),
            vec![48, 72]
        );
        // Both are a tenth of a second, so both count 4410 frames.
        assert_eq!(draft.takes[0].frames, 4_410);
        assert_eq!(draft.takes[1].frames, 4_410);
        assert_eq!(
            draft.takes[1].pcm().unwrap().rate,
            22_050,
            "the file's own rate is kept"
        );
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

    /// The same pick made into an instrument: the audio itself, one zone per file, and
    /// bytes that round-trip.
    #[test]
    fn a_draft_becomes_an_instrument_over_the_same_files() {
        use nord_format::formats::nsmp::encode::MIN_FRAMES;

        let draft = Draft::plan(
            Making::Instrument,
            vec![
                ("Marimba-C3.wav".into(), wav(SOURCE_RATE, MIN_FRAMES)),
                ("Marimba-C5.wav".into(), wav(SOURCE_RATE, MIN_FRAMES * 2)),
            ],
        )
        .expect("two files were picked");
        assert_eq!(draft.name, "Marimba-C3");
        assert!(draft.refusal().is_none());

        let bytes = draft.bytes().expect("an instrument");
        let entity = nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).unwrap();
        assert!(matches!(entity, Entity::Sample(_)), "{entity:?}");
        assert_eq!(nord_format::to_bytes(&entity).unwrap(), bytes);

        let snapshot = crate::document::sample::snapshot(&entity)
            .expect("a sample instrument")
            .expect("it reads");
        assert_eq!(snapshot.name, "Marimba-C3");
        assert_eq!(snapshot.generation, "v2");
        let zones: Vec<(u8, u8)> = snapshot
            .zones
            .iter()
            .map(|zone| (zone.root_key, zone.top_note))
            .collect();
        // High to low, and the top notes are the ones a project derives for these roots.
        assert_eq!(zones, [(72, 96), (48, 59)]);
    }

    /// ⚠️ The instrument carries the audio, so the dialog refuses what the encoder would
    /// refuse before Create, instead of failing after it. A project references the file
    /// and accepts it as it is.
    #[test]
    fn a_wav_the_encoder_will_not_take_is_refused_before_create() {
        let slow = vec![("Marimba-C3.wav".into(), wav(22_050, 4_410))];
        let project = Draft::plan(Making::Project, slow.clone()).unwrap();
        assert!(project.refusal().is_none(), "a project keeps the rate");

        let instrument = Draft::plan(Making::Instrument, slow).unwrap();
        let why = instrument.refusal().expect("22 050 Hz is not encodable");
        assert!(why.contains("22050 Hz"), "{why}");
        assert!(why.starts_with("Marimba-C3.wav: "), "{why}");
    }

    /// An instrument's name field holds 31 bytes, and a filename can be longer.
    #[test]
    fn an_instrument_opens_on_a_name_its_own_field_holds() {
        let long = ["an extremely long marimba sample name.wav".to_string()];
        assert_eq!(draft_name(Making::Instrument, &long).len(), MAX_NAME_LEN);
        assert!(draft_name(Making::Project, &long).len() > MAX_NAME_LEN);

        let mut draft = Draft::plan(
            Making::Instrument,
            vec![(
                "Marimba.wav".into(),
                wav(SOURCE_RATE, nord_format::formats::nsmp::encode::MIN_FRAMES),
            )],
        )
        .unwrap();
        draft.name = "x".repeat(MAX_NAME_LEN + 1);
        let why = draft.refusal().expect("a name the field cannot hold");
        assert!(why.contains(&MAX_NAME_LEN.to_string()), "{why}");
    }

    /// The project lands in a folder of its own, beside a copy of each WAV under a leaf
    /// of its own: two picks of one name are numbered apart, and the project names each
    /// by its leaf, while two picks of the same bytes share a file.
    #[test]
    fn a_new_project_is_filed_in_a_folder_of_its_own_beside_its_wavs() {
        let mut workspace = Workspace::new(crate::testing::context());
        let mut folders = Folders::default();
        let mut log = Log::default();
        folders.named_or_made(&LibPath::root(), "Marimba", &workspace);
        let (low, high) = (wav(44_100, 4_410), wav(22_050, 2_205));
        let mut draft = Draft::plan(
            Making::Project,
            vec![
                ("c3.wav".into(), low.clone()),
                ("C3.wav".into(), high.clone()),
                ("c3.wav".into(), low.clone()),
            ],
        )
        .unwrap();
        draft.name = "Marimba".into();
        let id = draft.make(&mut workspace, &mut folders, &mut log).unwrap();

        let dir = LibPath::parse("Marimba 2").unwrap();
        assert!(folders.id_of(&dir).is_some(), "a folder of its own");
        let mut filed: Vec<(String, Vec<u8>)> = workspace
            .listed()
            .filter(|entity| entity.path.as_ref().is_some_and(|at| at.parent() == dir))
            .map(|entity| (entity.name.clone(), entity.bytes.to_vec()))
            .collect();
        filed.sort();
        let names: Vec<&str> = filed.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["C3 2.wav", "Marimba.nsmpproj", "c3.wav"]);
        assert!(
            filed[0].1 == high && filed[2].1 == low,
            "each holds its pick"
        );

        let Some(Entity::SampleProject(project)) = workspace.get(id).unwrap().entity.as_deref()
        else {
            panic!("a Sample Editor project");
        };
        let mut paths: Vec<String> = project
            .audio_files()
            .unwrap()
            .into_iter()
            .map(|file| file.path)
            .collect();
        paths.sort();
        paths.dedup();
        assert_eq!(paths, ["C3 2.wav", "c3.wav"]);
    }

    #[test]
    fn a_canceled_pick_raises_no_dialog() {
        assert!(Draft::plan(Making::Project, Vec::new()).is_none());
        assert!(Draft::plan(Making::Instrument, Vec::new()).is_none());
    }

    /// A short mono take at the rate the piano section plays, so a build resamples
    /// nothing and the encoder has something other than silence to fit.
    fn stroke_wav(frames: usize) -> Vec<u8> {
        let samples: Vec<i16> = (0..frames)
            .map(|n| ((n as f64 * 0.05).sin() * 8_000.0) as i16)
            .collect();
        nord_format::wav::mono_pcm16(&samples, nord_format::formats::npno::codec::RATE).unwrap()
    }

    fn piano_draft(stems: &[&str]) -> Draft {
        let files = stems
            .iter()
            .map(|stem| (format!("{stem}.wav"), stroke_wav(3_000)))
            .collect();
        Draft::plan(Making::Piano, files).expect("files were picked")
    }

    fn finish(draft: &mut Draft) -> Result<Built, String> {
        loop {
            if let Some(answer) = draft.settle() {
                return answer;
            }
            std::thread::yield_now();
        }
    }

    #[test]
    fn a_wavs_name_states_the_stroke_it_holds() {
        assert_eq!(
            stroke_name("060-b0-l00.wav"),
            Some((60, Bank::Attack, LayerTag::Index(0))),
            "the spelling `nord piano build` reads"
        );
        assert_eq!(
            stroke_name("Grand-060-b0-l00.wav"),
            Some((60, Bank::Attack, LayerTag::Index(0))),
            "and the name this app exports a stroke under"
        );
        assert_eq!(stroke_name("060-b0.wav"), None, "no stroke named at all");

        assert_eq!(
            stroke_defaults("Marimba-C3.wav"),
            (48, Bank::Attack, LayerTag::Index(0)),
            "a plain name states a root and nothing else"
        );
        assert_eq!(
            stroke_defaults("hit.wav"),
            (MIDDLE_C, Bank::Attack, LayerTag::Index(0)),
            "and a name stating none defaults to middle C"
        );
    }

    /// A layer's value decides which velocity selects it, so one root's layers are spread
    /// over the velocity range instead of packed at the loud end.
    #[test]
    fn a_roots_layers_are_spread_over_the_velocity_range_loudest_first() {
        let draft = piano_draft(&["060-b0-l02", "060-b0-l00", "060-b0-l01", "072-b0-l00"]);
        assert_eq!(
            layer_values(&draft.takes).unwrap(),
            vec![27, 0, 14, 0],
            "the list order is kept; the rank is the layer's own"
        );
    }

    #[test]
    fn one_roots_bank_names_its_layers_one_way() {
        let draft = piano_draft(&["060-b0-l00", "060-b0-v12"]);
        let why = draft.refusal().expect("an index and a value in one group");
        assert!(why.contains("C4"), "{why}");
        assert!(why.contains("attack"), "{why}");

        let apart = piano_draft(&["060-b0-l00", "060-b2-v12"]);
        assert!(
            apart.refusal().is_none(),
            "a bank of its own names its layers its own way"
        );
    }

    /// Two takes claiming one layer of a root and bank would be spread to two different
    /// values, which is neither of the things their names said.
    #[test]
    fn one_roots_bank_names_each_of_its_layers_once() {
        let twice = piano_draft(&["a-060-b0-l00", "b-060-b0-l00"]);
        let why = twice.refusal().expect("one layer named twice");
        assert!(why.contains("C4"), "{why}");
        assert!(why.contains("names one of its layers twice"), "{why}");

        let apart = piano_draft(&["a-060-b0-l00", "b-060-b1-l00"]);
        assert!(
            apart.refusal().is_none(),
            "a bank of its own numbers its own layers"
        );
    }

    #[test]
    fn a_piano_draft_codes_the_takes_into_a_library() {
        let mut draft = piano_draft(&["Grand-060-b0-l00", "Grand-060-b0-l01", "Grand-072-b2-l00"]);
        assert_eq!(
            draft.name, "Grand",
            "the stroke suffix is not part of the name"
        );
        assert!(draft.refusal().is_none());

        draft
            .begin(&egui::Context::default(), None)
            .expect("the takes state a library");
        let built = finish(&mut draft).expect("a library");
        assert_eq!((built.strokes, built.roots), (3, 2));
        assert_eq!(built.clipped, 0);

        let entity = nord_format::from_stream(&mut std::io::Cursor::new(&built.bytes)).unwrap();
        assert!(matches!(entity, Entity::Piano(_)), "{entity:?}");
        assert_eq!(nord_format::to_bytes(&entity).unwrap(), built.bytes);
        let library = Library::borrow(&built.bytes).unwrap();
        assert_eq!(library.name(), ("Grand".to_string(), String::new()));
        let strokes: Vec<(u8, Option<Bank>, u8)> = library
            .strokes()
            .iter()
            .map(|stroke| (stroke.root, stroke.bank(), stroke.layer()))
            .collect();
        assert_eq!(
            strokes,
            [
                (60, Some(Bank::Attack), 0),
                (60, Some(Bank::Attack), SOFTEST_LAYER),
                (72, Some(Bank::Release), 0),
            ],
            "the root's two layers spread from the loudest to the softest"
        );
    }

    /// A template supplies the playback fields a recording cannot carry, and is read
    /// without its audio because a build cannot borrow the document's bytes.
    #[test]
    fn a_template_donates_what_the_recordings_do_not_state() {
        let mut plain = piano_draft(&["Grand-060-b0-l00"]);
        plain.begin(&egui::Context::default(), None).unwrap();
        let rules = finish(&mut plain).expect("a library").bytes;
        assert_eq!(
            Library::borrow(&rules).unwrap().damper_top(),
            Kind::Grand.damper_top()
        );

        let mut edited = Library::borrow(&rules).unwrap();
        edited.set_damper_top(100).unwrap();
        let template = nord_format::to_bytes(&Entity::Piano(edited.to_piano().unwrap())).unwrap();

        let mut again = piano_draft(&["Grand-060-b0-l00"]);
        let donor = Library::borrow(&template).unwrap().without_audio();
        again.begin(&egui::Context::default(), Some(donor)).unwrap();
        let built = finish(&mut again).expect("a library");
        assert_eq!(Library::borrow(&built.bytes).unwrap().damper_top(), 100);
    }

    /// ⚠️ The encoder reports the rules the dialog does not check, in its own words, with
    /// the takes still there.
    #[test]
    fn what_the_coder_refuses_comes_back_as_the_dialogs_own_line() {
        let mut draft = piano_draft(&["a-060-b0-v31", "b-072-b0-v31"]);
        assert!(draft.refusal().is_none(), "the dialog does not check this");

        draft.begin(&egui::Context::default(), None).unwrap();
        let why = finish(&mut draft).expect_err("a layer no velocity selects");
        assert!(why.contains("no velocity selects"), "{why}");
        assert_eq!(draft.refused.as_deref(), Some(why.as_str()));
        assert_eq!(draft.takes.len(), 2, "the takes stay, to be fixed");
    }

    #[test]
    fn a_draft_says_why_it_cannot_be_made() {
        let unreadable = Draft::plan(
            Making::Project,
            vec![("notes.txt".into(), b"not a wav".to_vec())],
        )
        .unwrap();
        let why = unreadable.refusal().expect("it will not read");
        assert!(why.starts_with("notes.txt: "), "{why}");

        let mut clashing = Draft::plan(
            Making::Project,
            vec![
                ("a.wav".into(), wav(44_100, 4_410)),
                ("b.wav".into(), wav(44_100, 4_410)),
            ],
        )
        .unwrap();
        assert!(clashing.refusal().is_none());
        clashing.takes[1].root_key = clashing.takes[0].root_key;
        let why = clashing.refusal().expect("two zones on one key");
        assert!(why.contains("C4"), "{why}");
        assert!(clashing.bytes().is_err());
    }
}
