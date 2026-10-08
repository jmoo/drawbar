//! A map for the many small maps of a merged state: a vector sorted by key,
//! which holds one or two members in one allocation, moved into a B-tree once it
//! holds [`SPILL`] so that a large one stays logarithmic.

use std::borrow::Borrow;
use std::collections::{btree_map, BTreeMap};
use std::fmt;

/// The members a vector holds before the map moves them into a B-tree.
const SPILL: usize = 32;

#[derive(Clone)]
pub(crate) enum SmallMap<K, V> {
    Few(Vec<(K, V)>),
    // Boxed, so the map is no wider than its vector.
    #[allow(clippy::box_collection)]
    Many(Box<BTreeMap<K, V>>),
}

pub(crate) type SmallSet<K> = SmallMap<K, ()>;

impl<K, V> Default for SmallMap<K, V> {
    fn default() -> Self {
        Self::Few(Vec::new())
    }
}

impl<K: Ord, V> SmallMap<K, V> {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Few(few) => few.len(),
            Self::Many(many) => many.len(),
        }
    }

    pub(crate) fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        match self {
            Self::Few(few) => few
                .binary_search_by(|(k, _)| k.borrow().cmp(key))
                .ok()
                .map(|at| &few[at].1),
            Self::Many(many) => many.get(key),
        }
    }

    pub(crate) fn contains_key<Q: Ord + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.get(key).is_some()
    }

    pub(crate) fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        match self {
            Self::Few(few) => few
                .binary_search_by(|(k, _)| k.borrow().cmp(key))
                .ok()
                .map(move |at| &mut few[at].1),
            Self::Many(many) => many.get_mut(key),
        }
    }

    /// The value under `key`, first inserting the pair `new` makes when there is
    /// none; `new` makes a pair under `key`.
    pub(crate) fn slot<Q: Ord + ?Sized>(&mut self, key: &Q, new: impl FnOnce() -> (K, V)) -> &mut V
    where
        K: Borrow<Q>,
    {
        if !self.contains_key(key) {
            let (key, value) = new();
            self.insert(key, value);
        }
        self.get_mut(key).expect("inserted above")
    }

    /// Inserts `value` under `key`; the value it replaces, if any.
    pub(crate) fn insert(&mut self, key: K, value: V) -> Option<V> {
        if let Some(held) = self.get_mut(&key) {
            return Some(std::mem::replace(held, value));
        }
        if let Self::Few(few) = self {
            if few.len() >= SPILL {
                let many = std::mem::take(few).into_iter().collect();
                *self = Self::Many(Box::new(many));
            }
        }
        match self {
            Self::Few(few) => {
                let at = few.partition_point(|(k, _)| *k < key);
                // Most maps keep one member: doubled from one, not from four.
                if few.len() == few.capacity() {
                    few.reserve_exact(few.len().max(1));
                }
                few.insert(at, (key, value));
            }
            Self::Many(many) => {
                many.insert(key, value);
            }
        }
        None
    }

    pub(crate) fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        match self {
            Self::Few(few) => few
                .binary_search_by(|(k, _)| k.borrow().cmp(key))
                .ok()
                .map(|at| few.remove(at).1),
            Self::Many(many) => many.remove(key),
        }
    }

    pub(crate) fn iter(&self) -> Iter<'_, K, V> {
        match self {
            Self::Few(few) => Iter::Few(few.iter()),
            Self::Many(many) => Iter::Many(many.iter()),
        }
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(key, _)| key)
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, value)| value)
    }
}

impl<K: Ord, V> SmallMap<K, V> {
    /// The map of `pairs`, which must be in strictly increasing order of key.
    pub(crate) fn from_sorted(pairs: Vec<(K, V)>) -> Option<Self> {
        if !pairs.windows(2).all(|pair| pair[0].0 < pair[1].0) {
            return None;
        }
        Some(match pairs.len() < SPILL {
            true => Self::Few(pairs),
            false => Self::Many(Box::new(pairs.into_iter().collect())),
        })
    }
}

impl<K: Ord> SmallSet<K> {
    /// Adds `key`; whether it was absent.
    pub(crate) fn add(&mut self, key: K) -> bool {
        self.insert(key, ()).is_none()
    }
}

impl<K: Ord, V> FromIterator<(K, V)> for SmallMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(pairs: I) -> Self {
        let mut map = Self::default();
        for (key, value) in pairs {
            map.insert(key, value);
        }
        map
    }
}

impl<K: Ord> FromIterator<K> for SmallSet<K> {
    fn from_iter<I: IntoIterator<Item = K>>(keys: I) -> Self {
        keys.into_iter().map(|key| (key, ())).collect()
    }
}

impl<K: Ord> Extend<K> for SmallSet<K> {
    fn extend<I: IntoIterator<Item = K>>(&mut self, keys: I) {
        for key in keys {
            self.add(key);
        }
    }
}

/// Equal when they hold the same pairs, however they hold them.
impl<K: Ord, V: PartialEq> PartialEq for SmallMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl<K: Ord, V: Eq> Eq for SmallMap<K, V> {}

impl<K: Ord + fmt::Debug, V: fmt::Debug> fmt::Debug for SmallMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

pub(crate) enum Iter<'a, K, V> {
    Few(std::slice::Iter<'a, (K, V)>),
    Many(btree_map::Iter<'a, K, V>),
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Few(few) => few.next().map(|(key, value)| (key, value)),
            Self::Many(many) => many.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Few(few) => few.size_hint(),
            Self::Many(many) => many.size_hint(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{Random, SeededRandom};

    #[test]
    fn a_small_map_holds_what_a_btree_map_holds() {
        let mut random = SeededRandom::new(3);
        for round in 0..200 {
            let range = 1 + (random.next_u128() % 200) as u32;
            let mut small: SmallMap<u32, u32> = SmallMap::default();
            let mut tree = BTreeMap::new();
            for step in 0..400 {
                let key = (random.next_u128() % u128::from(range)) as u32;
                match random.next_u128() % 4 {
                    0 => assert_eq!(small.remove(&key), tree.remove(&key)),
                    1 => assert_eq!(small.insert(key, step), tree.insert(key, step)),
                    _ => {
                        *small.slot(&key, || (key, 0)) += 1;
                        *tree.entry(key).or_insert(0) += 1;
                    }
                }
                assert_eq!(small.get(&key), tree.get(&key), "round {round}");
            }
            assert!(small.iter().eq(tree.iter()), "round {round}");
            assert_eq!(small.len(), tree.len());
            let rebuilt: SmallMap<u32, u32> = tree.iter().map(|(k, v)| (*k, *v)).collect();
            assert_eq!(small, rebuilt, "round {round}: equal however held");
        }
    }
}
