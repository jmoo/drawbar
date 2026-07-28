use crate::common::bank::{Item, Location};
use std::fmt::Debug;

#[derive(Debug)]
pub struct Song<const PROGRAM_COUNT: usize, SongLocation, ProgramLocation>
where
    SongLocation: Location,
    ProgramLocation: Location,
{
    name: Option<String>,
    location: SongLocation,
    programs: [ProgramLocation; PROGRAM_COUNT],
    /// Schema version from the container header. Carried so a song can be written back
    /// as the version it was read as: the eight factory demo songs are version 0 and
    /// everything user-written is version 1, and re-emitting a 0 as a 1 silently
    /// rewrites the file.
    version: u32,
}

impl<const C: usize, S, P> Song<C, S, P>
where
    S: Location,
    P: Location,
{
    pub fn new(location: S, programs: [P; C]) -> Song<C, S, P> {
        Song {
            name: None,
            location,
            programs,
            version: Self::DEFAULT_VERSION,
        }
    }

    /// What a newly authored song is written as. Reading a file overwrites this with
    /// whatever the file carried.
    pub const DEFAULT_VERSION: u32 = 1;

    /// Container schema version — see the field docs.
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn set_version(&mut self, version: u32) {
        self.version = version;
    }

    pub fn get(&self, slot: u16) -> P {
        self.programs[slot as usize]
    }

    pub fn set(&mut self, slot: u16, location: P) {
        self.programs[slot as usize] = location;
    }

    pub fn programs(&self) -> &[P; C] {
        &self.programs
    }
}

impl<const C: usize, S, P> Item<S> for Song<C, S, P>
where
    S: Location,
    P: Location,
{
    fn name(&self) -> Option<String> {
        self.name.clone()
    }

    fn set_name(&mut self, name: String) {
        self.name = Some(name);
    }

    fn location(&self) -> S {
        self.location
    }

    fn set_location(&mut self, location: S) {
        self.location = location;
    }
}
