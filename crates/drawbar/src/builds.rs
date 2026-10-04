//! Building a Sample Editor project in the library into the sample instrument it
//! describes, beside it.
//!
//! A project's WAVs are WAV assets of the library. [`locate`] finds each one when
//! [`Builds::start`] runs, and [`Builds::poll`] reads them, encodes their bytes on a
//! [`Job`], then files the instrument beside the project and opens it.

use std::borrow::Cow;
use std::collections::BTreeMap;

use nord_format::formats::nsmp::codec::Layout;
use nord_format::formats::nsmp::encode::Predictor;
use nord_format::formats::nsmpproj::build::{self, AudioPath, Unavailable, Warning};
use nord_format::formats::nsmpproj::{AudioFile, Project};
use nord_format::Entity;

use crate::browser::Kind;
use crate::folders::Folders;
use crate::log::Log;
use crate::store::{names, LibPath};
use crate::tabs::Tabs;
use crate::work::{self, Answer, Job};
use crate::workspace::{Bytes, LocalEntity, Origin, VerifyState, Workspace};

/// The generation a build writes.
pub const LAYOUT: Layout = Layout::V2;

/// Where one of a project's WAVs is in the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wav {
    /// The WAV asset at this path, by its id.
    Listed(LibPath, u64),
    /// Absolute, or climbing past the library's root. It is never followed.
    Outside,
    /// Listed files whose paths differ from the stored one only by [`names::key`].
    Ambiguous(Vec<LibPath>),
    Missing,
}

impl Wav {
    /// Why a build cannot read it, or `None` where it can.
    pub fn trouble(&self) -> Option<String> {
        match self {
            Wav::Listed(..) => None,
            Wav::Outside => Some("outside the library".to_string()),
            Wav::Ambiguous(paths) => {
                let paths: Vec<&str> = paths.iter().map(LibPath::as_str).collect();
                Some(format!("ambiguous between {}", paths.join(" and ")))
            }
            Wav::Missing => Some("missing".to_string()),
        }
    }
}

/// The library's WAV assets, by path, each with its id.
pub fn wav_assets(workspace: &Workspace) -> Vec<(LibPath, u64)> {
    let mut wavs: Vec<(LibPath, u64)> = workspace
        .listed()
        .filter(|entity| Kind::of(entity) == Kind::Wav)
        .filter_map(|entity| Some((entity.path.clone()?, entity.id)))
        .collect();
    wavs.sort();
    wavs
}

/// Where a project in `dir` finds the WAV it stores as `stored`, among the library's
/// WAV assets: the one at that exact path, or else the one whose path has the same
/// [`names::key`], which is how the library tells two names apart.
pub fn locate(dir: &LibPath, stored: &str, wavs: &[(LibPath, u64)]) -> Wav {
    let Some(parts) = AudioPath::parse(stored).within(dir.components()) else {
        return Wav::Outside;
    };
    let wanted = parts.join("/");
    if let Some((path, id)) = wavs.iter().find(|(path, _)| path.as_str() == wanted) {
        return Wav::Listed(path.clone(), *id);
    }
    let key = names::key(&wanted);
    let alike: Vec<&(LibPath, u64)> = wavs
        .iter()
        .filter(|(path, _)| names::key(path.as_str()) == key)
        .collect();
    match alike.as_slice() {
        [] => Wav::Missing,
        [(path, id)] => Wav::Listed(path.clone(), *id),
        _ => Wav::Ambiguous(alike.iter().map(|(path, _)| path.clone()).collect()),
    }
}

/// Each of a project's audio files by its stored path, with where its WAV is.
pub fn locate_all(
    dir: &LibPath,
    files: &[AudioFile],
    wavs: &[(LibPath, u64)],
) -> Vec<(String, Wav)> {
    files
        .iter()
        .map(|file| (file.path.clone(), locate(dir, &file.path, wavs)))
        .collect()
}

/// Each WAV that does not resolve, as `stored path: why`.
pub fn unresolved(wavs: &[(String, Wav)]) -> Vec<String> {
    wavs.iter()
        .filter_map(|(stored, wav)| Some(format!("{stored}: {}", wav.trouble()?)))
        .collect()
}

/// The folder a project's file is in, which its relative WAV paths start from.
pub fn dir_of(project: &crate::workspace::LocalEntity) -> LibPath {
    project
        .path
        .as_ref()
        .map_or_else(LibPath::root, LibPath::parent)
}

/// The name the instrument built from a project file named `project` is filed under.
pub fn instrument_name(project: &str) -> String {
    let stem = project.rsplit_once('.').map_or(project, |(stem, _)| stem);
    format!("{stem}.{}", LAYOUT.extension())
}

/// The bytes of each of a project's WAVs, by its stored path.
struct Read(BTreeMap<String, Bytes>);

impl build::Source for Read {
    fn wav(&self, file: &AudioFile) -> Result<Cow<'_, [u8]>, Unavailable> {
        self.0
            .get(&file.path)
            .map(|wav| Cow::Borrowed(&wav[..]))
            .ok_or(Unavailable::Missing)
    }
}

/// What an encode made.
struct Built {
    bytes: Vec<u8>,
    zones: usize,
    warnings: Vec<Warning>,
}

enum Stage {
    /// Waiting for the WAVs to be read: the project as it was asked to build, and the
    /// WAV asset each audio file's WAV is, by its stored path.
    Reading {
        project: Project,
        wavs: BTreeMap<String, (LibPath, u64)>,
    },
    Encoding(Job<Result<Built, String>>),
}

struct Running {
    /// The project file's name, which the log and the instrument's name take.
    name: String,
    stage: Stage,
}

/// The builds in flight, one at most per project.
#[derive(Default)]
pub struct Builds {
    running: BTreeMap<u64, Running>,
}

impl Builds {
    /// Whether the project `id` is building.
    pub fn running(&self, id: u64) -> bool {
        self.running.contains_key(&id)
    }

    /// Build the project `id` from what it holds now, its unsaved edits included, and
    /// look for its WAVs again. Refuses, in the log, a project that does not decode or
    /// names a WAV the library does not list.
    pub fn start(&mut self, id: u64, workspace: &Workspace, log: &mut Log) {
        let Some(entity) = workspace.get(id) else {
            return;
        };
        let name = entity.name.clone();
        if self.running(id) {
            return log.say(format!("“{name}” is building already."));
        }
        let Some(Entity::SampleProject(project)) = entity.entity.as_deref() else {
            return refuse(log, &name, "it does not decode as a Sample Editor project");
        };
        let files = match build::played(project) {
            Ok(files) => files,
            Err(e) => return refuse(log, &name, &e.to_string()),
        };
        let located = locate_all(&dir_of(entity), &files, &wav_assets(workspace));
        let unresolved = unresolved(&located);
        if !unresolved.is_empty() {
            return refuse(log, &name, &unresolved.join("; "));
        }
        let wavs = located
            .into_iter()
            .filter_map(|(stored, wav)| match wav {
                Wav::Listed(path, at) => Some((stored, (path, at))),
                Wav::Outside | Wav::Ambiguous(_) | Wav::Missing => None,
            })
            .collect();
        let stage = Stage::Reading {
            project: project.clone(),
            wavs,
        };
        self.running.insert(id, Running { name, stage });
    }

    /// Move each build along: hurry the WAVs not read yet, encode once all are, and file
    /// each instrument encoded beside its project, open in a tab. A build whose project
    /// is gone is dropped.
    pub fn poll(
        &mut self,
        workspace: &mut Workspace,
        folders: &Folders,
        tabs: &mut Tabs,
        log: &mut Log,
    ) {
        self.running.retain(|id, _| workspace.get(*id).is_some());
        let reading: Vec<u64> = self
            .running
            .iter()
            .filter(|(_, running)| matches!(running.stage, Stage::Reading { .. }))
            .map(|(id, _)| *id)
            .collect();
        for id in reading {
            self.read(id, workspace, log);
        }
        let mut answered = Vec::new();
        for (id, running) in &self.running {
            if let Stage::Encoding(job) = &running.stage {
                match job.poll() {
                    Answer::Running => {}
                    Answer::Answered(built) => answered.push((*id, built)),
                    Answer::Died => answered.push((*id, Err("the encode stopped".to_string()))),
                }
            }
        }
        for (id, built) in answered {
            let Some(running) = self.running.remove(&id) else {
                continue;
            };
            let Some(dir) = workspace.get(id).map(dir_of) else {
                continue;
            };
            match built {
                Ok(built) => land(&running.name, dir, built, workspace, folders, tabs, log),
                Err(why) => refuse(log, &running.name, &why),
            }
        }
    }

    /// Start encoding the project `id` once each of its WAVs is read, as its asset holds
    /// it now; hurry the ones not read yet. Refuses the build where a WAV is gone or could
    /// not be read.
    fn read(&mut self, id: u64, workspace: &Workspace, log: &mut Log) {
        let Some(Running {
            stage: Stage::Reading { wavs, .. },
            ..
        }) = self.running.get(&id)
        else {
            return;
        };
        let mut read = BTreeMap::new();
        let mut waiting = false;
        let mut refused = None;
        for (stored, (path, at)) in wavs {
            // Needed every frame, read or not, so making room for one never evicts another.
            workspace.hurry(*at);
            match held(workspace.get(*at)) {
                Held::Read(bytes) => _ = read.insert(stored.clone(), bytes),
                Held::Unread => waiting = true,
                Held::Not(why) => {
                    refused = Some(format!("{path}: {why}"));
                    break;
                }
            }
        }
        if waiting && refused.is_none() {
            return;
        }
        let Some(Running {
            name,
            stage: Stage::Reading { project, .. },
        }) = self.running.remove(&id)
        else {
            return;
        };
        if let Some(why) = refused {
            return refuse(log, &name, &why);
        }
        let source = Read(read);
        let job = work::run(workspace.ctx(), move |_| encode(&project, &source));
        let stage = Stage::Encoding(job);
        self.running.insert(id, Running { name, stage });
    }
}

/// What a build finds of one of its WAVs.
enum Held {
    /// Its bytes as the asset holds them now, an unsaved edit included.
    Read(Bytes),
    Unread,
    /// Why a build cannot have them.
    Not(String),
}

/// What a build can have of the WAV asset `entity`.
fn held(entity: Option<&LocalEntity>) -> Held {
    let Some(entity) = entity else {
        return Held::Not("it is gone".to_string());
    };
    if let VerifyState::NotRead(why) = &entity.verify {
        return Held::Not(why.clone());
    }
    if entity.unread() {
        return Held::Unread;
    }
    // A WAV is read whole: the store leaves only indexed instruments resting in their file.
    Held::Read(entity.bytes.clone())
}

/// The instrument `project` describes, from WAVs `source` holds.
fn encode(project: &Project, source: &Read) -> Result<Built, String> {
    let plan = build::plan(project, LAYOUT, source).map_err(|e| e.to_string())?;
    let sample = plan
        .encode(&plan.name, Predictor::Minimizing, None)
        .map_err(|e| e.to_string())?;
    Ok(Built {
        bytes: sample.to_bytes().map_err(|e| e.to_string())?,
        zones: plan.zones.len(),
        warnings: plan.warnings(),
    })
}

/// File what a build made beside its project, under the project's name numbered until it
/// is free, and open it.
fn land(
    project: &str,
    dir: LibPath,
    built: Built,
    workspace: &mut Workspace,
    folders: &Folders,
    tabs: &mut Tabs,
    log: &mut Log,
) {
    let name = folders.free(&dir, &instrument_name(project), workspace);
    let made = workspace.ingest(name.clone(), Origin::Fresh, built.bytes, log);
    workspace.place(made, dir.join(&name));
    tabs.open(made);
    for warning in built.warnings {
        log.warn(format!("{name}: {warning}"));
    }
    log.say(format!(
        "Built “{name}” from “{project}”: {} zone(s).",
        built.zones
    ));
}

fn refuse(log: &mut Log, project: &str, why: &str) {
    log.error(format!("building {project}: {why}"));
    log.trouble(format!("“{project}” was not built."));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> LibPath {
        LibPath::parse(text).unwrap()
    }

    /// WAV assets at these paths, numbered from 1 in order.
    fn listed(paths: &[&str]) -> Vec<(LibPath, u64)> {
        paths.iter().map(|text| path(text)).zip(1..).collect()
    }

    /// The asset at `text` among `wavs`, as [`listed`] numbers them.
    fn at(wavs: &[&str], text: &str) -> Wav {
        let id = wavs.iter().position(|held| *held == text).unwrap() as u64 + 1;
        Wav::Listed(path(text), id)
    }

    fn marimba(stored: &str, wavs: &[&str]) -> Wav {
        locate(&path("Marimba"), stored, &listed(wavs))
    }

    #[test]
    fn a_wav_is_found_at_the_path_the_project_stores_from_its_folder() {
        let wavs = ["Marimba/audio/c3.wav", "Shared/c4.wav"];
        let found = at(&wavs, "Marimba/audio/c3.wav");
        assert_eq!(marimba("audio/c3.wav", &wavs), found);
        assert_eq!(marimba(r"audio\c3.wav", &wavs), found);
        assert_eq!(marimba("./audio/c3.wav", &wavs), found);
    }

    #[test]
    fn a_climb_that_stays_inside_the_library_is_followed() {
        let wavs = ["Marimba/audio/c3.wav", "Shared/c4.wav"];
        assert_eq!(
            marimba("../Shared/c4.wav", &wavs),
            at(&wavs, "Shared/c4.wav")
        );
    }

    #[test]
    fn a_wav_named_in_another_case_is_found_under_its_listed_name() {
        let wavs = ["Marimba/Audio/c3.wav"];
        assert_eq!(
            marimba("audio/C3.wav", &wavs),
            at(&wavs, "Marimba/Audio/c3.wav")
        );
    }

    #[test]
    fn the_exact_path_wins_over_one_in_another_case() {
        let wavs = ["Marimba/C3.wav", "Marimba/c3.wav"];
        assert_eq!(marimba("c3.wav", &wavs), at(&wavs, "Marimba/c3.wav"));
    }

    #[test]
    fn two_listed_files_only_a_case_apart_are_ambiguous() {
        let wavs = ["Marimba/C3.wav", "Marimba/c3.wav"];
        let found = marimba("C3.WAV", &wavs);
        assert_eq!(found, Wav::Ambiguous(wavs.map(path).into()));
        assert_eq!(
            found.trouble().as_deref(),
            Some("ambiguous between Marimba/C3.wav and Marimba/c3.wav")
        );
    }

    #[test]
    fn a_path_outside_the_library_is_never_followed() {
        let wavs = ["c4.wav", "Marimba/c4.wav"];
        assert_eq!(marimba("../../c4.wav", &wavs), Wav::Outside);
        assert_eq!(marimba("/Users/jo/c4.wav", &wavs), Wav::Outside);
        assert_eq!(marimba(r"C:\Samples\c4.wav", &wavs), Wav::Outside);
        assert_eq!(marimba("..", &wavs), Wav::Outside);
    }

    #[test]
    fn a_wav_the_library_does_not_list_is_missing() {
        assert_eq!(
            marimba("audio/c5.wav", &["Marimba/audio/c3.wav"]),
            Wav::Missing
        );
        assert_eq!(Wav::Missing.trouble().as_deref(), Some("missing"));
    }

    #[test]
    fn a_project_looks_for_its_wavs_among_the_placed_wav_assets() {
        let mut log = Log::default();
        let mut workspace = Workspace::new(crate::testing::context());
        let mut placed = |name: &str, bytes: Vec<u8>, at: Option<&str>| {
            let id = workspace.ingest(name.into(), Origin::Fresh, bytes, &mut log);
            if let Some(at) = at {
                workspace.place(id, path(at));
            }
            id
        };
        let wav = crate::testing::wav_bytes;
        let c4 = placed("c4.WAV", wav(), Some("Shared/c4.WAV"));
        let c3 = placed("c3.wav", wav(), Some("Marimba/c3.wav"));
        placed("c5.wav", wav(), None);
        placed("notes.txt", b"Set 1\n".to_vec(), Some("Marimba/notes.txt"));
        assert_eq!(
            wav_assets(&workspace),
            [(path("Marimba/c3.wav"), c3), (path("Shared/c4.WAV"), c4)]
        );
    }

    #[test]
    fn the_instrument_takes_the_project_files_name() {
        assert_eq!(instrument_name("Marimba.nsmpproj"), "Marimba.nsmp");
        assert_eq!(instrument_name("Marimba v2.nsmpproj"), "Marimba v2.nsmp");
    }
}
