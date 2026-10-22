//! The decoded contents of an Electro 5 bundle or backup archive.

use crate::bank::Entry;
use crate::bundle::archive::{copy_member, Directory};
use crate::bundle::manifest;
use crate::cbin::Cbin;
use crate::error::{try_with_capacity, Error};
use crate::formats::ne5::{program, song};
use crate::formats::npno::Piano;
use crate::formats::nsmp::Sample;
use crate::{from_stream, Entity, Program, Song};
use std::io::{Read, Seek};

/// An Electro 5 bundle or backup, walked from its ZIP archive: banked
/// programs and songs, plus the piano and sample libraries they use.
#[derive(Debug, Default)]
pub struct Bundle {
    programs: program::Bank,
    songs: song::Bank,
    pianos: Vec<Piano>,
    samples: Vec<Cbin<Sample>>,
    /// Archive members the walk could not place, as `(member name, reason)`.
    skipped: Vec<(String, String)>,
}

impl Bundle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads a stored ZIP of the shape [`crate::bundle::archive`] accepts, checking every
    /// member's bytes against its entry. A member that does not decode is skipped, not
    /// refused.
    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Bundle, Error> {
        let mut bundle = Bundle::new();
        let directory = Directory::read_from(reader)?;

        for member in &directory.members {
            let name = member.entry.name.clone();
            let mut buffer = try_with_capacity(member.entry.size as usize, "bytes")?;
            copy_member(reader, member, &mut buffer)?;
            if name == manifest::PATH {
                continue;
            }
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
                    // Named by identity: a stub entity's Debug output is its entire body.
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

/// Records the member a later one pushed out of its slot.
///
/// A bank holds one item per slot, and each file names its slot. When two members name
/// the same slot, the last one read wins and the earlier one is listed as skipped.
fn note_displaced<T>(skipped: &mut Vec<(String, String)>, displaced: Option<Entry<T>>, by: &str) {
    let Some(entry) = displaced else { return };
    skipped.push((
        entry
            .name
            .unwrap_or_else(|| "an unnamed member".to_string()),
        format!("{by} claims the same slot"),
    ));
}
