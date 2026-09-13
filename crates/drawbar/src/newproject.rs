//! New → a Sample Editor project, a sample instrument or a piano library: some WAVs,
//! what each one was recorded at, and the `.nsmpproj`, `.nsmp` or `.npno` that comes
//! out of them.
//!
//! The editor's own *Import Auto…* is what the first two imitate — one zone per file,
//! ordered by root key, key ranges derived from the roots. Everything either format
//! needs beyond the audio is the root key, and a filename is the only place a guess at
//! one can come from, so the dialog exists to let that guess be corrected before
//! anything is made.
//!
//! A piano library asks for more per file — a bank and a velocity layer as well as a
//! root — and takes long enough to code that the build runs off the frame.
//!
//! ⚠️ A project holds **paths, not audio**. What lands in the list references the WAVs
//! by the names they were picked under, and the editor looks for them beside the
//! project file. An instrument and a library hold the audio itself, which is why they
//! take the encoder's limits on what a WAV may be.

use std::collections::BTreeMap;

use eframe::egui;
use nord_format::formats::npno::encode::{
    build, layer_value, resample, Donor, Kind, Options, Recording, Rules,
};
use nord_format::formats::npno::{Bank, Library};
use nord_format::formats::nsmp::codec::{Layout, SOURCE_RATE};
use nord_format::formats::nsmp::zone::derive_top_notes;
use nord_format::formats::nsmp::{encode, MAX_NAME_LEN};
use nord_format::formats::nsmpproj::{NewZone, Project, HIGHEST_NOTE, LOWEST_NOTE};
use nord_format::wav::Pcm16;
use nord_format::Entity;

use crate::document::encode::{fits, refusal as encodable, Source};
use crate::document::note_picker;
use crate::log::Log;
use crate::note;
use crate::work::{self, Job, Progress};
use crate::workspace::{Origin, Workspace};

/// Zones one draft can hold: every key the dialog lays a root on.
const MOST_ZONES: usize = (HIGHEST_NOTE - LOWEST_NOTE) as usize + 1;

/// The root a file that names none is taken to have been recorded at.
const MIDDLE_C: u8 = 60;

/// The top of the scale a layer value is selected on — see [`Stroke::layer`]. A value
/// past [`nord_format::formats::npno::encode::HIGHEST_PLAYED_LAYER`] is one `build`
/// refuses, in its own words.
///
/// [`Stroke::layer`]: nord_format::formats::npno::Stroke::layer
const TOP_LAYER: u8 = 31;

/// What a pick of WAVs is turned into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Making {
    /// A `.nsmpproj`: the file names, and where each sits on the keyboard.
    Project,
    /// A `.nsmp`: the audio itself, one zone per file, at the generation that has been
    /// played on hardware. The document panel over a WAV is where a generation is
    /// chosen; this makes the one that plays.
    Instrument,
    /// A `.npno`: one stroke per file, resampled onto the lattice the piano section
    /// plays at and coded into the library.
    Piano,
}

impl Making {
    /// Everything a pick of WAVs makes, in the order the New menu offers them.
    pub const FROM_WAVS: [Making; 3] = [Making::Project, Making::Instrument, Making::Piano];

    /// What it makes, as the dialog, the file picker and the log name it.
    pub fn label(self) -> &'static str {
        match self {
            Making::Project => "Sample Editor project",
            Making::Instrument => "sample instrument",
            Making::Piano => "piano library",
        }
    }

    /// The New menu's own item, and what hovering it says.
    pub fn item(self) -> (&'static str, &'static str) {
        match self {
            Making::Project => (
                "Sample Editor project…",
                "pick the WAVs it plays; the project stores their names and the editor \
                 looks for them beside it",
            ),
            Making::Instrument => (
                "Sample instrument…",
                "pick the WAVs it plays; the audio is encoded into the instrument, so \
                 the files are not needed afterwards",
            ),
            Making::Piano => (
                "Piano library…",
                "pick one WAV per stroke; the audio is coded into the library, so the \
                 files are not needed afterwards",
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
                "One zone per file, ordered by root key. The project stores the file \
                 names and the editor looks for them beside it, so keep them together."
            }
            Making::Instrument => {
                "One zone per file, ordered by root key. The audio is encoded into the \
                 instrument, so the WAVs are not needed afterwards."
            }
            Making::Piano => {
                "One stroke per WAV. A name like 060-b0-l00.wav sets the root, bank and \
                 layer; set them here otherwise. Kind, gain and the damper limit are \
                 edited in the document afterwards."
            }
        }
    }
}

/// What a WAV's name says about the velocity layer its stroke sits at.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum LayerTag {
    /// `l02`: the third-loudest layer of its root and bank, taking whatever value the
    /// spread over that root's layers gives it.
    Index(u8),
    /// `v12`: the layer value itself, written to the record as it stands.
    Value(u8),
}

impl LayerTag {
    fn number(self) -> u8 {
        match self {
            LayerTag::Index(n) | LayerTag::Value(n) => n,
        }
    }

    fn word(self) -> &'static str {
        match self {
            LayerTag::Index(_) => "index",
            LayerTag::Value(_) => "value",
        }
    }

    fn with(self, n: u8) -> LayerTag {
        match self {
            LayerTag::Index(_) => LayerTag::Index(n),
            LayerTag::Value(_) => LayerTag::Value(n),
        }
    }
}

/// One picked WAV, as the dialog shows it.
pub struct Take {
    /// The name a project would reference it by, resolved beside the project file.
    pub path: String,
    /// The file as it read, or the reader's own complaint.
    pub source: Source,
    /// Frames as a project counts them — see [`at_source_rate`]. Zero where the file
    /// did not read, or holds no audio.
    pub frames: u64,
    pub root_key: u8,
    /// Which of a piano library's three banks the stroke belongs to. A zone has no
    /// such thing.
    pub bank: Bank,
    /// Where the stroke sits among its root and bank's layers. A zone has no such
    /// thing.
    pub layer: LayerTag,
}

impl Take {
    fn new(path: String, bytes: &[u8], root_key: u8, bank: Bank, layer: LayerTag) -> Take {
        let source = Source::read(bytes);
        let frames = match &source {
            Source::Read(pcm) => at_source_rate(pcm.frames() as u64, pcm.rate).unwrap_or_default(),
            Source::Unreadable(_) => 0,
        };
        Take {
            path,
            source,
            frames,
            root_key,
            bank,
            layer,
        }
    }

    fn pcm(&self) -> Option<&Pcm16> {
        match &self.source {
            Source::Read(pcm) => Some(pcm),
            Source::Unreadable(_) => None,
        }
    }

    /// Why this file cannot be part of what is being made.
    ///
    /// A project references audio it never reads, so anything that holds some will do.
    /// An instrument carries it, and so takes the encoder's own limits. A library takes
    /// any rate — it resamples — and states the rest of its limits when it is coded.
    pub fn refusal(&self, making: Making) -> Option<String> {
        match making {
            Making::Instrument => encodable(&self.source),
            Making::Project => match &self.source {
                Source::Unreadable(why) => Some(why.clone()),
                Source::Read(_) => (self.frames == 0).then(|| "it holds no audio".to_string()),
            },
            Making::Piano => match &self.source {
                Source::Unreadable(why) => Some(why.clone()),
                Source::Read(_) => None,
            },
        }
    }
}

/// The picked files, waiting on their root keys.
pub struct Draft {
    pub making: Making,
    pub name: String,
    pub takes: Vec<Take>,
    /// The piano document a build donates its playback fields from, where one is
    /// chosen.
    pub template: Option<u64>,
    /// The build in flight, once Create has been pressed.
    job: Option<Job<Result<Built, String>>>,
    /// What the coder last refused, kept beside the takes so they can be fixed.
    refused: Option<String>,
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

/// The stroke a WAV's name states: `060-b0-l00`, as `nord piano build` reads it, and
/// `<stem>-060-b0-l00` as [`crate::workspace::stroke_wav_name`] writes it.
///
/// The trailing group is the whole claim, so a name carrying anything of its own in
/// front of it still names its stroke.
fn stroke_name(path: &str) -> Option<(u8, Bank, LayerTag)> {
    let stem = path.rsplit_once('.').map_or(path, |(stem, _)| stem);
    let mut parts = stem.rsplit('-');
    let third = parts.next()?;
    let layer = match (third.strip_prefix('l'), third.strip_prefix('v')) {
        (Some(index), _) => LayerTag::Index(index.parse().ok()?),
        (None, Some(value)) => LayerTag::Value(value.parse().ok()?),
        (None, None) => return None,
    };
    let bank = Bank::from_code(parts.next()?.strip_prefix('b')?.parse().ok()?)?;
    Some((parts.next()?.parse().ok()?, bank, layer))
}

/// Whether a dropped file is one an open draft takes rather than a document to open.
/// The extension alone: a file that will not read is listed with the reader's own
/// complaint beside it.
pub fn is_wav_name(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
}

/// What a picked WAV is taken to be a recording of: the stroke its name states, or a
/// root read off the end of the name and the loudest layer of the attack bank.
fn stroke_defaults(path: &str) -> (u8, Bank, LayerTag) {
    stroke_name(path).unwrap_or((
        trailing_note(path).unwrap_or(MIDDLE_C),
        Bank::Attack,
        LayerTag::Index(0),
    ))
}

/// The name a draft over these files starts under: the first file's stem, cut to what
/// an instrument's own name field holds.
fn draft_name(making: Making, paths: &[String]) -> String {
    let first = paths.first().map(String::as_str).unwrap_or_default();
    let stem = first.rsplit_once('.').map_or(first, |(stem, _)| stem);
    // A stroke's own name is the stroke, not the library: `Grand-060-b0-l00` opens on
    // `Grand`.
    let stem = match making == Making::Piano && stroke_name(stem).is_some() {
        true => stem.rsplitn(4, '-').nth(3).unwrap_or_default(),
        false => stem,
    };
    let stem = match stem.trim() {
        "" => "Untitled",
        stem => stem,
    };
    match making {
        Making::Project | Making::Piano => stem.to_string(),
        Making::Instrument => fits(stem),
    }
}

impl Draft {
    /// What was picked, read for what a project needs to know about it.
    ///
    /// Nothing is refused whole here: a file that will not read is still listed, with
    /// the reason beside it, because the operator picked it on purpose.
    pub fn plan(making: Making, files: Vec<(String, Vec<u8>)>) -> Option<Draft> {
        if files.is_empty() {
            return None;
        }
        let paths: Vec<String> = files.iter().map(|(name, _)| name.clone()).collect();
        let takes = match making {
            Making::Piano => files
                .iter()
                .map(|(path, bytes)| {
                    let (root, bank, layer) = stroke_defaults(path);
                    Take::new(path.clone(), bytes, root, bank, layer)
                })
                .collect(),
            Making::Project | Making::Instrument => files
                .iter()
                .zip(default_roots(&paths))
                .map(|((path, bytes), root_key)| {
                    Take::new(
                        path.clone(),
                        bytes,
                        root_key,
                        Bank::Attack,
                        LayerTag::Index(0),
                    )
                })
                .collect(),
        };
        Some(Draft {
            making,
            name: draft_name(making, &paths),
            takes,
            template: None,
            job: None,
            refused: None,
        })
    }

    /// Take on more files, as a drop onto the open dialog does.
    pub fn add(&mut self, files: Vec<(String, Vec<u8>)>) {
        for (path, bytes) in files {
            let take = match self.making {
                Making::Piano => {
                    let (root, bank, layer) = stroke_defaults(&path);
                    Take::new(path, bytes.as_slice(), root, bank, layer)
                }
                Making::Project | Making::Instrument => {
                    let root = trailing_note(&path).unwrap_or_else(|| self.free_key());
                    Take::new(
                        path,
                        bytes.as_slice(),
                        root,
                        Bank::Attack,
                        LayerTag::Index(0),
                    )
                }
            };
            self.takes.push(take);
        }
    }

    /// The lowest key a project maps that no take is on: where a dropped file naming
    /// no key goes, since each zone needs a key of its own.
    fn free_key(&self) -> u8 {
        (LOWEST_NOTE..=HIGHEST_NOTE)
            .find(|key| self.takes.iter().all(|take| take.root_key != *key))
            .unwrap_or(HIGHEST_NOTE)
    }

    /// Why this draft cannot be made into anything yet, in the operator's words.
    pub fn refusal(&self) -> Option<String> {
        if let Some((take, why)) = self
            .takes
            .iter()
            .find_map(|take| Some((take, take.refusal(self.making)?)))
        {
            return Some(format!("{}: {why}", take.path));
        }
        // ⚠️ A library states every other rule about its strokes when it is coded, and
        // says so in its own words with the takes still here to be fixed.
        if self.making == Making::Piano {
            return layer_values(&self.takes).err();
        }
        if self.making == Making::Instrument && self.name.len() > MAX_NAME_LEN {
            return Some(format!(
                "the name is {} bytes — an instrument's own name field holds \
                 {MAX_NAME_LEN}",
                self.name.len()
            ));
        }
        if self.takes.len() > MOST_ZONES {
            return Some(format!(
                "{} files — one zone per file, and the dialog lays roots on the \
                 {MOST_ZONES} keys a project maps",
                self.takes.len()
            ));
        }
        if let Some(take) = self
            .takes
            .iter()
            .find(|take| !(LOWEST_NOTE..=HIGHEST_NOTE).contains(&take.root_key))
        {
            return Some(format!(
                "{} is set to {} — the keys run {} to {}",
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

    /// The file the picked WAVs make, in the frame that asks for it.
    ///
    /// ⚠️ Coding a library takes longer than a frame, so the dialog runs it off one —
    /// [`Draft::begin`]. This is the same build with nothing to report progress to.
    fn bytes(&self) -> Result<Vec<u8>, String> {
        match self.making {
            Making::Project => self.project(),
            Making::Instrument => self.instrument(),
            Making::Piano => self
                .coding(None)?
                .run(&Progress::default())
                .map(|built| built.bytes),
        }
    }

    /// Everything a build needs, owned, so it can run away from the dialog.
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

    /// Start coding the library, donating from `donor` where a template was chosen.
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

    /// The answer the build has ready, taken once. A refusal stays behind as the line
    /// the dialog paints.
    fn settle(&mut self) -> Option<Result<Built, String>> {
        let answer = self.job.as_ref()?.poll()?;
        self.job = None;
        if let Err(why) = &answer {
            self.refused = Some(why.clone());
        }
        Some(answer)
    }

    fn project(&self) -> Result<Vec<u8>, String> {
        let zones: Vec<NewZone> = self
            .takes
            .iter()
            .map(|take| NewZone {
                path: take.path.clone(),
                sample_rate: take.pcm().map_or(0, |pcm| pcm.rate),
                frames: take.frames,
                root_key: take.root_key,
            })
            .collect();
        let project = Project::new(&self.name, &zones, now()).map_err(|e| e.to_string())?;
        nord_format::to_bytes(&Entity::SampleProject(project)).map_err(|e| e.to_string())
    }

    /// One `stk` per file, highest root first, each zone reaching up to where
    /// [`derive_top_notes`] puts it — the layout `Project::new` writes, encoded rather
    /// than referenced.
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
                predictor: encode::Predictor::Minimising,
                layout: Layout::V2,
                preset: encode::Preset::default(),
            },
            &zones,
        )
        .map_err(|e| e.to_string())?;
        instrument.to_bytes().map_err(|e| e.to_string())
    }
}

/// One take as a build reads it: the audio as the file holds it, and the stroke it is
/// to become.
struct Recorded {
    path: String,
    samples: Vec<i16>,
    channels: u16,
    rate: u32,
    root: u8,
    bank: Bank,
    layer: u8,
}

/// A piano library's build, away from the dialog that stated it.
struct Coding {
    name: String,
    donor: Option<Library<'static>>,
    takes: Vec<Recorded>,
}

/// A library coded, and what the coding has to say about it.
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

/// The layer value each take's stroke states, in the order the takes are listed.
///
/// A [`LayerTag::Value`] is that value; a [`LayerTag::Index`] is spread across its root
/// and bank's own layers, loudest first. The two forms would each mean something
/// different about how many layers a spread is over, so one root's bank names its
/// layers one way.
fn layer_values(takes: &[Take]) -> Result<Vec<u8>, String> {
    let mut groups: BTreeMap<(u8, Bank), Vec<usize>> = BTreeMap::new();
    for (index, take) in takes.iter().enumerate() {
        groups
            .entry((take.root_key, take.bank))
            .or_default()
            .push(index);
    }

    let mut values = vec![0u8; takes.len()];
    for ((root, bank), mut members) in groups {
        let stated = members
            .iter()
            .filter(|&&i| matches!(takes[i].layer, LayerTag::Value(_)))
            .count();
        if stated != 0 && stated != members.len() {
            return Err(format!(
                "root {} {} names some of its layers by index and some by value; one \
                 root's bank names them one way",
                note::name(root),
                bank.name(),
            ));
        }
        members.sort_by_key(|&i| takes[i].layer);
        let layers = members.len();
        for (rank, index) in members.into_iter().enumerate() {
            values[index] = match takes[index].layer {
                LayerTag::Value(value) => value,
                LayerTag::Index(_) => layer_value(rank, layers),
            };
        }
    }
    Ok(values)
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

/// The dialog between picking WAVs and having what they make, if a draft is waiting.
///
/// Returns the new asset once it is made, for the caller to open a tab on.
pub fn dialog(ctx: &egui::Context, workspace: &mut Workspace, log: &mut Log) -> Option<u64> {
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
                                        take.frames as f64 / f64::from(SOURCE_RATE)
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
        // ⚠️ wasm runs the build where it is started, so the answer is already here.
        return answered(workspace, log);
    }
    let draft = workspace.take_draft()?;
    match draft.bytes() {
        Ok(bytes) => Some(workspace.ingest(
            format!("{}.{}", draft.name, making.extension()),
            Origin::Fresh,
            bytes,
            log,
        )),
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

/// The bank and layer a piano take carries beyond a zone's root key.
fn stroke_controls(ui: &mut egui::Ui, i: usize, take: &mut Take) {
    egui::ComboBox::from_id_salt(("draft_bank", i))
        .width(92.0)
        .selected_text(take.bank.name())
        .show_ui(ui, |ui| {
            for bank in Bank::ALL {
                ui.selectable_value(&mut take.bank, bank, bank.name());
            }
        });
    let number = take.layer.number();
    egui::ComboBox::from_id_salt(("draft_layer", i))
        .width(64.0)
        .selected_text(take.layer.word())
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut take.layer, LayerTag::Index(number), "index");
            ui.selectable_value(&mut take.layer, LayerTag::Value(number), "value");
        });
    let mut set = number;
    if ui
        .add(egui::DragValue::new(&mut set).range(0..=TOP_LAYER))
        .changed()
    {
        take.layer = take.layer.with(set);
    }
}

/// What "none" is called in the template picker, and the reading of an empty list.
const NO_TEMPLATE: &str = "none — the rules";

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
        .filter(|entity| matches!(entity.entity, Some(Entity::Piano(_))))
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
    Library::borrow(&entity.bytes)
        .map(|library| library.without_audio())
        .map_err(|e| format!("{}: {e}", entity.name))
}

/// Set the build going, with whatever template the Advanced row names.
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

/// What the build has to say for itself, once it has something to say.
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

    /// The same pick, made into the instrument instead: the audio itself, one zone per
    /// file, and bytes that come back the way they went out.
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

    /// ⚠️ The instrument carries the audio, so the encoder's own limits are refusals in
    /// the dialog rather than a failure after Create. A project references the file and
    /// takes it as it is.
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

    /// The name field an instrument carries is 31 bytes, and a filename is not.
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

    #[test]
    fn a_cancelled_pick_raises_no_dialog() {
        assert!(Draft::plan(Making::Project, Vec::new()).is_none());
        assert!(Draft::plan(Making::Instrument, Vec::new()).is_none());
    }

    // ---- a piano library ----------------------------------------------------------

    /// A short mono take at the rate the piano section plays, so a build resamples
    /// nothing and the coder has something other than silence to fit.
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
            "and the one a stroke exported from here is written under"
        );
        assert_eq!(
            stroke_name("101-b1-v12.wav"),
            Some((101, Bank::Resonance, LayerTag::Value(12)))
        );
        assert_eq!(stroke_name("060-b3-l00.wav"), None, "no such bank");
        assert_eq!(stroke_name("060-b0-x2.wav"), None, "no such layer form");
        assert_eq!(stroke_name("060-b0.wav"), None);

        assert_eq!(
            stroke_defaults("Marimba-C3.wav"),
            (48, Bank::Attack, LayerTag::Index(0)),
            "a plain name states a root and nothing else"
        );
        assert_eq!(
            stroke_defaults("hit.wav"),
            (MIDDLE_C, Bank::Attack, LayerTag::Index(0)),
            "and a name stating none opens on middle C"
        );
    }

    /// The value a layer takes is what selects it, so the layers of one root are spread
    /// over the velocity range rather than packed at the loud end.
    #[test]
    fn a_roots_layers_are_spread_over_the_velocity_range_loudest_first() {
        let draft = piano_draft(&["060-b0-l02", "060-b0-l00", "060-b0-l01", "072-b0-l00"]);
        assert_eq!(
            layer_values(&draft.takes).unwrap(),
            vec![
                layer_value(2, 3),
                layer_value(0, 3),
                layer_value(1, 3),
                layer_value(0, 1),
            ],
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

    #[test]
    fn a_piano_draft_codes_the_takes_into_a_library() {
        let mut draft = piano_draft(&["Grand-060-b0-l00", "Grand-060-b0-l01", "Grand-072-b2-l00"]);
        assert_eq!(draft.name, "Grand", "the stroke group is not the name");
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
                (60, Some(Bank::Attack), layer_value(0, 2)),
                (60, Some(Bank::Attack), layer_value(1, 2)),
                (72, Some(Bank::Release), 0),
            ]
        );
    }

    /// A template donates the playback a recording cannot carry, and is read without
    /// its audio because a build cannot borrow the document's bytes.
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

    /// ⚠️ Everything a library states about its strokes beyond the one rule the dialog
    /// holds is stated by the coder, in its own words, with the takes still there.
    #[test]
    fn what_the_coder_refuses_comes_back_as_the_dialogs_own_line() {
        let mut draft = piano_draft(&["a-060-b0-v03", "b-060-b0-v03"]);
        assert!(
            draft.refusal().is_none(),
            "the dialog states no rule about this"
        );

        draft.begin(&egui::Context::default(), None).unwrap();
        let why = finish(&mut draft).expect_err("one stroke named twice");
        assert!(why.contains("recorded twice"), "{why}");
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
