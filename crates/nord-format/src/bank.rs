//! A bank of slot-addressed items, and the names the instrument gives them.

use std::collections::HashMap;

use std::fmt::{Debug, Formatter};
use std::hash::Hash;
use std::marker::PhantomData;

pub trait Location:
    Debug + Clone + Copy + PartialEq + Eq + Hash + TryFrom<u16> + TryFrom<(u16, u16)>
{
    fn inner(&self) -> (u16, u16);
    fn as_u16(&self) -> u16;
    fn x(&self) -> u16;
    fn y(&self) -> u16;
}

/// An item that knows which slot it occupies.
///
/// The slot lives only in the container header, so an implementation reads it from
/// there and keeps no copy in a field of its own.
pub trait Item<T>: Debug
where
    T: Location,
{
    fn location(&self) -> T;
}

/// One slot's occupant.
///
/// The name belongs to the bank, not the item. No file stores a name: it lives on the
/// instrument and arrives alongside the bytes, so the bank pairs it with the item.
#[derive(Debug)]
pub struct Entry<T> {
    pub name: Option<String>,
    pub item: T,
}

/// Slot-addressed items of one kind, each optionally carrying the name the
/// instrument shows for it.
pub struct Bank<T, L>
where
    L: Location,
    T: Item<L>,
{
    items: HashMap<u16, Entry<T>>,
    location_type: PhantomData<L>,
}

impl<T, L> Bank<T, L>
where
    L: Location,
    T: Item<L>,
{
    pub fn new() -> Bank<T, L> {
        Bank {
            items: HashMap::new(),
            location_type: PhantomData,
        }
    }

    /// Put `item` in the slot it claims, under `name`, returning whatever it displaced.
    ///
    /// A bank holds one item per slot. Returning the displaced entry lets a caller that
    /// walks several files decide what a displacement means.
    pub fn replace(&mut self, name: Option<String>, item: T) -> Option<Entry<T>> {
        self.items
            .insert(item.location().as_u16(), Entry { name, item })
    }

    pub fn get(&self, location: L) -> Option<&Entry<T>> {
        self.items.get(&location.as_u16())
    }

    /// How many slots hold an item.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl<T, L> Default for Bank<T, L>
where
    L: Location,
    T: Item<L>,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, L> Debug for Bank<T, L>
where
    L: Location,
    T: Item<L>,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if !self.items.is_empty() {
            writeln!(f, "Bank(")?;

            let mut keys: Vec<&u16> = self.items.keys().collect();
            keys.sort_unstable();
            for k in keys {
                let v = &self.items[k];
                match L::try_from(*k) {
                    Ok(location) => {
                        write!(f, "{}:{}\t{:?},\n\n", location.x() + 1, location.y() + 1, v)?
                    }
                    Err(_) => write!(f, "#{k}\t{v:?},\n\n")?,
                }
            }

            writeln!(f, ")")
        } else {
            write!(f, "Bank()")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Item;
    use crate::error::Error;
    use crate::types::RangedU16Pair;

    #[test]
    fn can_replace_items() -> Result<(), Error> {
        const BANK_COUNT: u16 = 5;
        const SLOT_COUNT: u16 = 2;

        type Location = RangedU16Pair<BANK_COUNT, SLOT_COUNT>;
        type Bank = crate::bank::Bank<TestItem, Location>;

        #[derive(Debug)]
        struct TestItem {
            pub location: Location,
            pub value: u16,
        }

        impl Item<Location> for TestItem {
            fn location(&self) -> Location {
                self.location
            }
        }

        let mut bank = Bank::new();

        let displaced = bank.replace(
            Some("foo".to_string()),
            TestItem {
                value: 69,
                location: (4, 1).try_into()?,
            },
        );
        assert!(displaced.is_none(), "an empty slot displaced something");

        if let Some(result) = bank.get((4, 1).try_into()?) {
            assert_eq!(result.item.value, 69);
            assert_eq!(result.name.as_deref(), Some("foo"));
        } else {
            panic!("Expected to find item at (4,1) but found nothing");
        }

        assert!(bank.get((0, 0).try_into()?).is_none());

        let displaced = bank
            .replace(
                Some("bar".to_string()),
                TestItem {
                    value: 70,
                    location: (4, 1).try_into()?,
                },
            )
            .expect("the occupied slot's previous entry");
        assert_eq!(displaced.name.as_deref(), Some("foo"));
        assert_eq!(displaced.item.value, 69);
        assert_eq!(bank.len(), 1, "a displaced item left a second slot behind");

        Ok(())
    }
}
