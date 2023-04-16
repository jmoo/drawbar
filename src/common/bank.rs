use crate::common::Error;
use crate::NordResult;
use std::fmt::{Debug, Formatter, Result};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Location {
    bank_size: u16,
    bank: u16,
    slot: u16,
    value: u16,
}

impl Location {
    pub fn from_coords(bank_size: u16, bank: u16, slot: u16) -> Location {
        Location {
            bank_size,
            bank,
            slot,
            value: bank * bank_size + slot,
        }
    }

    pub fn from_value(bank_size: u16, value: u16) -> Location {
        Location {
            bank_size,
            bank: value / bank_size,
            slot: value % bank_size,
            value,
        }
    }

    pub fn bank(&self) -> u16 {
        self.bank
    }

    pub fn slot(&self) -> u16 {
        self.slot
    }

    pub fn value(&self) -> u16 {
        self.value
    }

    pub fn coords(&self) -> (u16, u16) {
        (self.bank, self.slot)
    }

    pub fn update(&mut self, bank: u16, slot: u16) -> NordResult<()> {
        if slot > self.bank_size {
            return Err(Error::InvalidSchema);
        }

        self.bank = bank;
        self.slot = slot;
        self.value = bank * self.bank_size + slot;

        Ok(())
    }
}

impl Debug for Location {
    fn fmt(&self, f: &mut Formatter) -> Result {
        write!(f, "Location(#{}: {}, {})", self.value, self.bank, self.slot)
    }
}
