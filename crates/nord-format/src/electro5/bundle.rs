use crate::common::bank::Item;
use crate::common::piano::Piano;
use crate::common::sample::Sample;
use crate::electro5::{program, song};
use crate::{from_stream, Entity, Program, Song};
use binrw::BinReaderExt;
use std::io::Read;

#[derive(Debug)]
pub struct Bundle {
    programs: program::Bank,
    songs: song::Bank,
    pianos: Vec<Piano>,
    samples: Vec<Sample>,
    /// Entries the walk could not place: `(archive member name, why)`. Kept on the
    /// bundle rather than printed — a library owns no terminal — so a caller can decide
    /// whether a partial read is acceptable.
    skipped: Vec<(String, String)>,
    name: Option<String>,
}

impl Bundle {
    pub fn new() -> Self {
        Self {
            programs: program::Bank::new(),
            songs: song::Bank::new(),
            pianos: Vec::new(),
            samples: Vec::new(),
            skipped: Vec::new(),
            name: None,
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Bundle, std::io::Error> {
        let mut bundle = Bundle::new();

        let mut zip = zip::ZipArchive::new(reader)?;

        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            let name = file.name().to_string();

            let mut buffer: Vec<u8> = Vec::new();
            file.read_to_end(&mut buffer)?;
            let mut cursor = std::io::Cursor::new(buffer);

            match from_stream(&mut cursor) {
                Ok(entity) => match entity {
                    Entity::Program(Program::Electro5(mut program)) => {
                        program.set_name(name.clone());
                        bundle.programs.replace(program);
                    }
                    Entity::Song(Song::Electro5(mut song)) => {
                        song.set_name(name.clone());
                        bundle.songs.replace(song);
                    }
                    Entity::Piano(piano) => {
                        bundle.pianos.push(piano);
                    }
                    Entity::Sample(sample) => {
                        bundle.samples.push(sample);
                    }
                    other => bundle
                        .skipped
                        .push((name, format!("no place in a bundle for {other:?}"))),
                },
                Err(e) => bundle.skipped.push((name, e.to_string())),
            }
        }

        Ok(bundle)
    }

    pub fn name(&self) -> Option<String> {
        self.name.clone()
    }

    pub fn set_name(&mut self, name: String) {
        self.name = Some(name);
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

    pub fn samples(&self) -> &[Sample] {
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
