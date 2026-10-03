//! What a read of a file found, kept without its bytes.
//!
//! An asset whose file is not read this session still draws as it did once read: its
//! kind, its family, the slot it matches and the library it plays come from its
//! [`Summary`]. [`crate::store`] keeps summaries between sessions; see its cache.

use nord_format::Entity;
use nord_usb::ObjectClass;
use serde::{Deserialize, Serialize};

use crate::browser::Kind;
use crate::store::LibPath;
use crate::workspace::{LocalEntity, VerifyState};

/// What a row draws from a decode of an asset's saved bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub kind: Kind,
    /// The format tag, which names the family and model: `ne5p`. See
    /// [`LocalEntity::tag`].
    pub tag: String,
    /// The checksum a slot holding these bytes would report. See
    /// [`crate::workspace::Baseline::crc32`].
    pub crc32: Option<u32>,
    /// The library a program plays.
    pub plays: Option<Plays>,
    pub verdict: Verdict,
    /// For a sample instrument, its generation. See [`LocalEntity::generation`].
    pub generation: Option<&'static str>,
    /// For a Sample Editor project, the WAVs it names, as it names them with `/` between
    /// folders: relative to the project's folder in every project the editor writes.
    pub wavs: Vec<String>,
}

impl Summary {
    /// What `entity` holds, as a summary: the one it was given while it is unread, or one
    /// taken from its decode. `None` while it is still being read, decoded or checked, or
    /// could not be read.
    ///
    /// ⚠️ Describes what it holds now, an edit included. Only a clean asset's summary
    /// describes its file.
    pub fn of(entity: &LocalEntity) -> Option<Summary> {
        if entity.unread() {
            return entity.remembered.as_deref().cloned();
        }
        let verdict = Verdict::of(&entity.verify)?;
        let decoded = entity.entity.as_deref();
        Some(Summary {
            kind: Kind::of(entity),
            tag: entity.tag(),
            crc32: entity.saved.crc32,
            plays: entity.plays,
            verdict,
            generation: entity.generation(),
            wavs: match decoded {
                Some(Entity::SampleProject(project)) => project
                    .audio_files()
                    .map(|files| files.into_iter().map(|file| slashed(&file.path)).collect())
                    .unwrap_or_default(),
                _ => Vec::new(),
            },
        })
    }
}

/// Every generation [`LocalEntity::generation`] names, so a kept one reads back as the
/// same word.
pub const GENERATIONS: [&str; 3] = ["v2", "v3", "v4"];

/// The library a program plays: its class, and the id the instrument knows it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Plays {
    Piano(u32),
    Sample(u32),
}

impl Plays {
    /// The first library a program names, piano before sample.
    pub fn of(entity: &Entity) -> Option<Plays> {
        let fields = crate::fields::fields_of(entity)?;
        let named = |path: &str| {
            fields
                .iter()
                .find(|field| field.path == path)
                .and_then(|field| crate::document::library_id(&field.value))
                // Zero is "this program references no library", not an id to look for.
                .filter(|id| *id != 0)
        };
        named("piano_panel.id")
            .map(Plays::Piano)
            .or_else(|| named("sample_panel.id").map(Plays::Sample))
    }

    pub fn class(self) -> ObjectClass {
        match self {
            Plays::Piano(_) => ObjectClass::Piano,
            Plays::Sample(_) => ObjectClass::Sample,
        }
    }

    pub fn id(self) -> u32 {
        match self {
            Plays::Piano(id) | Plays::Sample(id) => id,
        }
    }
}

/// What checking a file found, as [`VerifyState`] says it, without the words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Ok,
    Checked,
    Differs { at: u64 },
    Failed,
    NotApplicable,
}

impl Verdict {
    /// The verdict a check reached, or `None` where none has been reached.
    pub fn of(verify: &VerifyState) -> Option<Verdict> {
        match verify {
            VerifyState::Ok => Some(Verdict::Ok),
            VerifyState::Checked => Some(Verdict::Checked),
            VerifyState::Differs { at } => Some(Verdict::Differs { at: *at as u64 }),
            VerifyState::Failed(_) => Some(Verdict::Failed),
            VerifyState::NotApplicable(_) => Some(Verdict::NotApplicable),
            VerifyState::Remembered(verdict) => Some(*verdict),
            VerifyState::Checking | VerifyState::Reading | VerifyState::NotRead(_) => None,
        }
    }
}

/// The projects that name one WAV.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Naming {
    /// The projects naming it, by id.
    pub by: Vec<u64>,
    /// The projects not read this session or before, which may name it.
    pub unknown: Vec<u64>,
}

/// A path as a project names it, with `/` between folders.
fn slashed(path: &str) -> String {
    path.replace('\\', "/")
}

/// The file a project in `dir` means by `named`: a path relative to its folder, `..`
/// and `.` resolved. `None` for an absolute path, or one that leaves the library.
pub fn resolve(dir: &LibPath, named: &str) -> Option<LibPath> {
    let named = slashed(named);
    if named.starts_with('/') || named.split('/').next()?.contains(':') {
        return None;
    }
    let mut parts: Vec<&str> = dir.components().collect();
    for part in named.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    LibPath::parse(&parts.join("/")).filter(|path| !path.is_root())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> LibPath {
        LibPath::parse(text).unwrap()
    }

    #[test]
    fn a_project_names_a_wav_relative_to_its_own_folder() {
        let dir = path("Marimba");
        assert_eq!(
            resolve(&dir, "audio/c4.wav"),
            Some(path("Marimba/audio/c4.wav"))
        );
        assert_eq!(
            resolve(&dir, r"audio\c4.wav"),
            Some(path("Marimba/audio/c4.wav"))
        );
        assert_eq!(resolve(&dir, "./c4.wav"), Some(path("Marimba/c4.wav")));
        assert_eq!(
            resolve(&dir, "../Shared/c4.wav"),
            Some(path("Shared/c4.wav"))
        );
        assert_eq!(resolve(&LibPath::root(), "c4.wav"), Some(path("c4.wav")));
    }

    #[test]
    fn a_wav_outside_the_library_resolves_to_nothing() {
        let dir = path("Marimba");
        assert_eq!(resolve(&dir, "../../c4.wav"), None);
        assert_eq!(resolve(&dir, "/Users/jo/c4.wav"), None);
        assert_eq!(resolve(&dir, r"C:\Samples\c4.wav"), None);
        assert_eq!(resolve(&dir, ".."), None);
    }
}
