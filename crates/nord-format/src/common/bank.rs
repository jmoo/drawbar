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

pub trait Item<T>: Debug
where
    T: Location,
{
    fn name(&self) -> Option<String>;
    fn set_name(&mut self, name: String);
    fn location(&self) -> T;
    fn set_location(&mut self, location: T);
}

pub struct Bank<T, L>
where
    L: Location,
    T: Item<L>,
{
    items: HashMap<u16, T>,
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

    pub fn replace(&mut self, item: T) {
        let id = item.location().as_u16();

        if self.items.contains_key(&id) {
            self.items.remove(&id);
        }

        self.items.insert(id, item);
    }

    pub fn get(&self, location: L) -> Option<&T> {
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
        type Bank = crate::common::bank::Bank<TestItem, Location>;

        #[derive(Debug)]
        struct TestItem {
            pub location: Location,
            pub value: u16,
        }

        impl Item<Location> for TestItem {
            fn name(&self) -> Option<String> {
                Some("foo".to_string())
            }
            fn set_name(&mut self, _name: String) {}
            fn location(&self) -> Location {
                self.location
            }
            fn set_location(&mut self, location: Location) {
                self.location = location;
            }
        }

        let mut bank = Bank::new();

        bank.replace(TestItem {
            value: 69,
            location: (4, 1).try_into()?,
        });

        if let Some(result) = bank.get((4, 1).try_into()?) {
            assert_eq!(result.value, 69);
        } else {
            panic!("Expected to find item at (4,1) but found nothing");
        }

        assert!(bank.get((0, 0).try_into()?).is_none());

        Ok(())
    }
}
