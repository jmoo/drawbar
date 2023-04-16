use crate::common::bank::Location;
use std::fmt::{Formatter, Result};

pub trait Song {
    fn location(&self) -> Location;

    fn programs(&self) -> Vec<Location>;

    fn fmt(&self, f: &mut Formatter) -> Result {
        write!(f, "Song({:?},{:?})", self.location(), self.programs())
    }
}
