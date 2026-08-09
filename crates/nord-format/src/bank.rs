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
/// The slot lives in the container header and nowhere else, so an implementation
/// reads it back out rather than shadowing it in a field of its own.
pub trait Item<T>: Debug
where
    T: Location,
{
    fn location(&self) -> T;
}

/// One slot's occupant.
///
/// The name is the bank's, not the item's: no file on disk stores a name — it lives
/// on the instrument and arrives alongside the bytes — so it is paired with the item
/// here rather than smuggled into the entity.
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

    /// Put `item` in the slot it claims, under `name`, displacing whatever was there.
    pub fn replace(&mut self, name: Option<String>, item: T) {
        self.items
            .insert(item.location().as_u16(), Entry { name, item });
    }

    pub fn get(&self, location: L) -> Option<&Entry<T>> {
        self.items.get(&location.as_u16())
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

            for (k, v) in self.items.iter() {
                let location: L = match (*k).try_into() {
                    Ok(l) => l,
                    Err(_e) => panic!("Failed to convert u16 to Location: {:?}", *k),
                };

                write!(f, "{}:{}\t{:?},\n\n", location.x() + 1, location.y() + 1, v)?;
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

        bank.replace(
            Some("foo".to_string()),
            TestItem {
                value: 69,
                location: (4, 1).try_into()?,
            },
        );

        if let Some(result) = bank.get((4, 1).try_into()?) {
            assert_eq!(result.item.value, 69);
            // The name came from the bank, not from the item.
            assert_eq!(result.name.as_deref(), Some("foo"));
        } else {
            panic!("Expected to find item at (4,1) but found nothing");
        }

        assert!(bank.get((0, 0).try_into()?).is_none());

        Ok(())
    }
}
