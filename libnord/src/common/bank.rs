

use std::collections::HashMap;
use std::convert::From;
use std::fmt::{Debug, Formatter, Result};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Coordinates<const BANK_COUNT: u16, const SLOT_COUNT: u16> {
    inner: (u16, u16),
}

impl<const BANK_COUNT: u16, const SLOT_COUNT: u16> Coordinates<BANK_COUNT, SLOT_COUNT> {
    pub fn from_value(value: u16) -> Coordinates<BANK_COUNT, SLOT_COUNT> {
        if value >= BANK_COUNT * SLOT_COUNT {
            panic!("Value out of bounds: {}", value)
        }

        Coordinates {
            inner: (value / SLOT_COUNT, value % SLOT_COUNT),
        }
    }

    pub fn from_coords(coords: (u16, u16)) -> Coordinates<BANK_COUNT, SLOT_COUNT> {
        if coords.0 >= BANK_COUNT || coords.1 >= SLOT_COUNT {
            panic!("Coordinates out of bounds: {:?}", coords)
        }

        Coordinates { inner: coords }
    }

    pub fn value(&self) -> u16 {
        self.inner.0 * SLOT_COUNT + self.inner.1
    }

    pub fn bank(&self) -> u16 {
        self.inner.0
    }

    pub fn slot(&self) -> u16 {
        self.inner.1
    }
}

impl<const B: u16, const S: u16> Debug for Coordinates<B, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "Coordinates({:?},{:?})", self.inner.0, self.inner.1)
    }
}

impl<const B: u16, const S: u16> From<(u16, u16)> for Coordinates<B, S> {
    fn from(coords: (u16, u16)) -> Self {
        Coordinates::<B, S>::from_coords(coords)
    }
}

impl<const B: u16, const S: u16> PartialEq<(u16, u16)> for Coordinates<B, S> {
    fn eq(&self, other: &(u16, u16)) -> bool {
        self.inner == *other
    }
}

pub trait Item<const BANK_COUNT: u16, const SLOT_COUNT: u16> {
    fn name(&self) -> Option<String>;
    fn set_name(&mut self, name: String) -> ();
    fn location(&self) -> Coordinates<BANK_COUNT, SLOT_COUNT>;
    fn set_location(&mut self, location: Coordinates<BANK_COUNT, SLOT_COUNT>) -> ();
}

pub struct Bank<
    const BANK_COUNT: u16,
    const SLOT_COUNT: u16,
    T: Item<BANK_COUNT, SLOT_COUNT> + Debug,
> {
    items: HashMap<u16, T>,
}

impl<const BANK_COUNT: u16, const SLOT_COUNT: u16, T> Bank<BANK_COUNT, SLOT_COUNT, T>
where
    T: Item<BANK_COUNT, SLOT_COUNT> + Debug,
{
    pub fn new() -> Bank<BANK_COUNT, SLOT_COUNT, T> {
        Bank {
            items: HashMap::new(),
        }
    }

    pub fn replace(&mut self, item: T) {
        let id = item.location().value();

        if self.items.contains_key(&id) {
            self.items.remove(&id);
        }

        self.items.insert(id, item);
    }

    pub fn get(&self, location: Coordinates<BANK_COUNT, SLOT_COUNT>) -> Option<&T> {
        self.items.get(&location.value())
    }
}

impl<const B: u16, const S: u16, T> Debug for Bank<B, S, T>
where
    T: Item<B, S> + Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "common::Bank(BANK_COUNT={}, SLOT_COUNT={}, ", B, S)?;

        for (k, v) in self.items.iter() {
            let coords = Coordinates::<B, S>::from_value(*k);
            write!(
                f,
                "Item(bank: {}, slot: {}, inner: {:?}\n",
                coords.bank(),
                coords.slot(),
                v
            )?;
            write!(f, "), ")?;
        }
        writeln!(f, ")")?;
        Ok(())
    }
}
