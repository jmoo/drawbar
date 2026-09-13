//! The decoded contents of an Electro 5 bundle or backup archive.

use crate::bank::Entry;
use crate::cbin::Cbin;
use crate::error::Error;
use crate::formats::ne5::{program, song};
use crate::formats::npno::Piano;
use crate::formats::nsmp::Sample;
use crate::{from_stream, Entity, Program, Song};
use std::io::{Read, Seek};

/// An Electro 5 bundle or backup, walked from its ZIP archive: banked
/// programs and songs, plus the piano and sample libraries they use.
#[derive(Debug)]
pub struct Bundle {
    programs: program::Bank,
    songs: song::Bank,
    pianos: Vec<Piano>,
    samples: Vec<Cbin<Sample>>,
    /// Entries the walk could not place: `(archive member name, why)`. Kept on the
    /// bundle rather than printed — a library owns no terminal — so a caller can decide
    /// whether a partial read is acceptable.
    skipped: Vec<(String, String)>,
}

impl Bundle {
    pub fn new() -> Self {
        Self {
            programs: program::Bank::new(),
            songs: song::Bank::new(),
            pianos: Vec::new(),
            samples: Vec::new(),
            skipped: Vec::new(),
        }
    }

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Bundle, Error> {
        let mut bundle = Bundle::new();

        let mut zip = zip::ZipArchive::new(reader)?;

        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            // A directory entry carries no file, and a backup manifest describes the
            // archive rather than being a member of it.
            if file.is_dir() || file.name().ends_with("meta.xml") {
                continue;
            }
            let name = file.name().to_string();

            let buffer = crate::formats::zip_member_bytes(&mut file)?;
            let mut cursor = std::io::Cursor::new(buffer);

            match from_stream(&mut cursor) {
                Ok(entity) => match entity {
                    // The archive member's name is the only name a bundle has for an
                    // entry: the file inside it stores none.
                    Entity::Program(Program::Electro5(program)) => {
                        let displaced = bundle.programs.replace(Some(name.clone()), program);
                        note_displaced(&mut bundle.skipped, displaced, &name);
                    }
                    Entity::Song(Song::Electro5(song)) => {
                        let displaced = bundle.songs.replace(Some(name.clone()), song);
                        note_displaced(&mut bundle.skipped, displaced, &name);
                    }
                    Entity::Piano(piano) => {
                        bundle.pianos.push(piano);
                    }
                    Entity::Sample(crate::Sample::V2(sample)) => {
                        bundle.samples.push(sample);
                    }
                    // Named by identity, not Debug — a stub entity's Debug dump
                    // is its entire body.
                    other => bundle.skipped.push((
                        name,
                        format!("no place in a bundle for a {}", other.identity().kind),
                    )),
                },
                Err(e) => bundle.skipped.push((name, e.to_string())),
            }
        }

        Ok(bundle)
    }

    pub fn programs(&self) -> &program::Bank {
        &self.programs
    }

    pub fn songs(&self) -> &song::Bank {
        &self.songs
    }

    pub fn pianos(&self) -> &[Piano] {
        &self.pianos
    }

    pub fn samples(&self) -> &[Cbin<Sample>] {
        &self.samples
    }

    /// Archive members that did not become entities, with the reason each was skipped.
    /// Empty means the whole bundle was understood.
    pub fn skipped(&self) -> &[(String, String)] {
        &self.skipped
    }
}

impl Default for Bundle {
    fn default() -> Self {
        Self::new()
    }
}

/// Report the member a later one pushed out of its slot.
///
/// A bank holds one item per slot and the file carries the slot, so two members
/// addressed to the same one cannot both be kept: the last read wins and the loser is
/// accounted for rather than dropped.
fn note_displaced<T>(skipped: &mut Vec<(String, String)>, displaced: Option<Entry<T>>, by: &str) {
    let Some(entry) = displaced else { return };
    skipped.push((
        entry
            .name
            .unwrap_or_else(|| "an unnamed member".to_string()),
        format!("{by} claims the same slot"),
    ));
}
