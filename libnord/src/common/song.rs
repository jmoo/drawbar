use crate::common::bank::{Coordinates};




pub trait Song<const BANK_COUNT: u16, const BANK_SIZE: u16> {
    fn get(&self, slot: u16) -> Coordinates<BANK_COUNT, BANK_SIZE>;
    fn set(&mut self, slot: u16, coords: Coordinates<BANK_COUNT, BANK_SIZE>) -> ();
}
