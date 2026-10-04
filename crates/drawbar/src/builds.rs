//! Building a Sample Editor project in the library into the sample instrument it
//! describes, beside it.
//!
//! A project's WAVs are files the library lists by name only. [`locate`] finds each one,
//! [`Builds::start`] asks the store for them, and [`Builds::poll`] encodes what it reads on
//! a [`Job`], then files the instrument beside the project and opens it.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use nord_format::formats::nsmp::codec::Layout;
use nord_format::formats::nsmp::encode::Predictor;
use nord_format::formats::nsmpproj::build::{self, AudioPath, Unavailable, Warning};
use nord_format::formats::nsmpproj::{AudioFile, Project};
use nord_format::Entity;

use crate::folders::Folders;
use crate::log::Log;
use crate::store::{names, Contents, Failure, LibPath, Store};
use crate::tabs::Tabs;
use crate::work::{self, Answer, Job};
use crate::workspace::{Origin, Workspace};

/// The generation a build writes.
pub const LAYOUT: Layout = Layout::V2;

/// Where one of a project's WAVs is in the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wav {
    Listed(LibPath),
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
            Wav::Listed(_) => None,
            Wav::Outside => Some("outside the library".to_string()),
            Wav::Ambiguous(paths) => {
                let paths: Vec<&str> = paths.iter().map(LibPath::as_str).collect();
                Some(format!("ambiguous between {}", paths.join(" and ")))
            }
            Wav::Missing => Some("missing".to_string()),
        }
    }
}

/// Where a project in `dir` finds the WAV it stores as `stored`, among the files the
/// library lists by name only: the file at that exact path, or else the one whose path
/// has the same [`names::key`], which is how the library tells two names apart.
pub fn locate(dir: &LibPath, stored: &str, others: &[LibPath]) -> Wav {
    let Some(parts) = AudioPath::parse(stored).within(dir.components()) else {
        return Wav::Outside;
    };
    let wanted = parts.join("/");
    if let Some(exact) = others.iter().find(|path| path.as_str() == wanted) {
        return Wav::Listed(exact.clone());
    }
    let key = names::key(&wanted);
    let mut alike: Vec<LibPath> = others
        .iter()
        .filter(|path| names::key(path.as_str()) == key)
        .cloned()
        .collect();
    match alike.len() {
        0 => Wav::Missing,
        1 => Wav::Listed(alike.remove(0)),
        _ => Wav::Ambiguous(alike),
    }
}

/// Each of a project's audio files by its stored path, with where its WAV is.
pub fn locate_all(dir: &LibPath, files: &[AudioFile], others: &[LibPath]) -> Vec<(String, Wav)> {
    files
        .iter()
        .map(|file| (file.path.clone(), locate(dir, &file.path, others)))
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

/// The project's WAVs as read from the library.
struct Read {
    /// The listed file each audio file's WAV is, by its stored path.
    wavs: BTreeMap<String, LibPath>,
    bytes: BTreeMap<LibPath, Vec<u8>>,
}

impl build::Source for Read {
    fn wav(&self, file: &AudioFile) -> Result<Cow<'_, [u8]>, Unavailable> {
        self.wavs
            .get(&file.path)
            .and_then(|at| self.bytes.get(at))
            .map(|wav| Cow::Borrowed(wav.as_slice()))
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
    /// Waiting for the WAVs: the project as it was asked to build, and the listed file
    /// each audio file's WAV is, by its stored path. `sent` once the store has been asked.
    Reading {
        project: Project,
        wavs: BTreeMap<String, LibPath>,
        sent: bool,
    },
    Encoding(Job<Result<Built, String>>),
}

struct Running {
    /// What the store's answer to this build's read is tied to.
    request: u64,
    /// The project file's name, which the log and the instrument's name take.
    name: String,
    stage: Stage,
}

/// The builds in flight, one at most per project.
#[derive(Default)]
pub struct Builds {
    next: u64,
    running: BTreeMap<u64, Running>,
}

impl Builds {
    /// Whether the project `id` is building.
    pub fn running(&self, id: u64) -> bool {
        self.running.contains_key(&id)
    }

    /// Build the project `id` from what it holds now, its unsaved edits included: look
    /// for its WAVs again and ask for them. Refuses, in the log, a project that does not
    /// decode or names a WAV the library does not list.
    pub fn start(&mut self, id: u64, workspace: &Workspace, folders: &Folders, log: &mut Log) {
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
        let files = match project.audio_files() {
            Ok(files) => files,
            Err(e) => return refuse(log, &name, &e.to_string()),
        };
        let located = locate_all(&dir_of(entity), &files, &folders.others);
        let unresolved = unresolved(&located);
        if !unresolved.is_empty() {
            return refuse(log, &name, &unresolved.join("; "));
        }
        let wavs = located
            .into_iter()
            .filter_map(|(stored, wav)| match wav {
                Wav::Listed(at) => Some((stored, at)),
                Wav::Outside | Wav::Ambiguous(_) | Wav::Missing => None,
            })
            .collect();
        self.next += 1;
        self.running.insert(
            id,
            Running {
                request: self.next,
                name,
                stage: Stage::Reading {
                    project: project.clone(),
                    wavs,
                    sent: false,
                },
            },
        );
    }

    /// Move each build along: ask the store for the WAVs, encode what it answered, and
    /// file each instrument encoded beside its project, open in a tab. A build whose
    /// project is gone is dropped.
    pub fn poll(
        &mut self,
        store: Option<&mut Store>,
        workspace: &mut Workspace,
        folders: &Folders,
        tabs: &mut Tabs,
        log: &mut Log,
    ) {
        self.running.retain(|id, _| workspace.get(*id).is_some());
        match store {
            Some(store) => {
                self.ask(store);
                for (request, files) in store.take_others() {
                    self.read(request, files, workspace.ctx(), log);
                }
            }
            None => self.running.retain(|_, running| match running.stage {
                Stage::Reading { .. } => {
                    refuse(
                        log,
                        &running.name,
                        "no library is open to read its WAVs from",
                    );
                    false
                }
                Stage::Encoding(_) => true,
            }),
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

    /// Ask the store for the WAVs of each build that has not asked yet.
    fn ask(&mut self, store: &mut Store) {
        for running in self.running.values_mut() {
            if let Stage::Reading { wavs, sent, .. } = &mut running.stage {
                if !*sent {
                    let paths: BTreeSet<LibPath> = wavs.values().cloned().collect();
                    store.read_others(running.request, paths.into_iter().collect());
                    *sent = true;
                }
            }
        }
    }

    /// Start encoding the build whose read `request` answered, or refuse it where a WAV
    /// did not read.
    fn read(&mut self, request: u64, files: Contents, ctx: &eframe::egui::Context, log: &mut Log) {
        let Some(id) = self
            .running
            .iter()
            .find(|(_, running)| {
                running.request == request && matches!(running.stage, Stage::Reading { .. })
            })
            .map(|(id, _)| *id)
        else {
            return;
        };
        let Some(running) = self.running.remove(&id) else {
            return;
        };
        let Stage::Reading { project, wavs, .. } = running.stage else {
            return;
        };
        let mut bytes = BTreeMap::new();
        for (path, read) in files {
            let why = match read {
                Ok(read) => {
                    bytes.insert(path, read);
                    continue;
                }
                Err(Failure::Moved) => "it is gone".to_string(),
                Err(Failure::Room(len)) => format!("its {len} bytes did not fit"),
                Err(Failure::Io(why)) => why,
            };
            return refuse(log, &running.name, &format!("{path}: {why}"));
        }
        let source = Read { wavs, bytes };
        let job = work::run(ctx, move |_| encode(&project, &source));
        self.running.insert(
            id,
            Running {
                stage: Stage::Encoding(job),
                ..running
            },
        );
    }
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
    log.trouble(format!("“{project}” was not built: {why}."));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> LibPath {
        LibPath::parse(text).unwrap()
    }

    fn listed(paths: &[&str]) -> Vec<LibPath> {
        paths.iter().map(|text| path(text)).collect()
    }

    fn marimba(stored: &str, others: &[&str]) -> Wav {
        locate(&path("Marimba"), stored, &listed(others))
    }

    #[test]
    fn a_wav_is_found_at_the_path_the_project_stores_from_its_folder() {
        let others = ["Marimba/audio/c3.wav", "Shared/c4.wav"];
        let found = Wav::Listed(path("Marimba/audio/c3.wav"));
        assert_eq!(marimba("audio/c3.wav", &others), found);
        assert_eq!(marimba(r"audio\c3.wav", &others), found);
        assert_eq!(marimba("./audio/c3.wav", &others), found);
    }

    #[test]
    fn a_climb_that_stays_inside_the_library_is_followed() {
        let others = ["Marimba/audio/c3.wav", "Shared/c4.wav"];
        assert_eq!(
            marimba("../Shared/c4.wav", &others),
            Wav::Listed(path("Shared/c4.wav"))
        );
    }

    #[test]
    fn a_wav_named_in_another_case_is_found_under_its_listed_name() {
        assert_eq!(
            marimba("audio/C3.wav", &["Marimba/Audio/c3.wav"]),
            Wav::Listed(path("Marimba/Audio/c3.wav"))
        );
    }

    #[test]
    fn the_exact_path_wins_over_one_in_another_case() {
        let others = ["Marimba/C3.wav", "Marimba/c3.wav"];
        assert_eq!(
            marimba("c3.wav", &others),
            Wav::Listed(path("Marimba/c3.wav"))
        );
    }

    #[test]
    fn two_listed_files_only_a_case_apart_are_ambiguous() {
        let others = ["Marimba/C3.wav", "Marimba/c3.wav"];
        let found = marimba("C3.WAV", &others);
        assert_eq!(found, Wav::Ambiguous(listed(&others)));
        assert_eq!(
            found.trouble().as_deref(),
            Some("ambiguous between Marimba/C3.wav and Marimba/c3.wav")
        );
    }

    #[test]
    fn a_path_outside_the_library_is_never_followed() {
        let others = ["c4.wav", "Marimba/c4.wav"];
        assert_eq!(marimba("../../c4.wav", &others), Wav::Outside);
        assert_eq!(marimba("/Users/jo/c4.wav", &others), Wav::Outside);
        assert_eq!(marimba(r"C:\Samples\c4.wav", &others), Wav::Outside);
        assert_eq!(marimba("..", &others), Wav::Outside);
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
    fn the_instrument_takes_the_project_files_name() {
        assert_eq!(instrument_name("Marimba.nsmpproj"), "Marimba.nsmp");
        assert_eq!(instrument_name("Marimba v2.nsmpproj"), "Marimba v2.nsmp");
    }
}
