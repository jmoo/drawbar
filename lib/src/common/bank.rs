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
    fn set_name(&mut self, name: String) -> ();
    fn location(&self) -> T;
    fn set_location(&mut self, location: T) -> ();
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

impl<T, L> Debug for Bank<T, L>
where
    L: Location,
    T: Item<L>,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.items.len() > 0 {
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
