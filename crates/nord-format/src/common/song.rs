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
        }
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
