//! A sorted map whose clones share their entries.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

/// The most entries one chunk holds.
const CHUNK: usize = 128;

type Chunk<K, V> = Arc<Vec<(K, V)>>;

/// A sorted map kept as chunks of at most [`CHUNK`] entries, each shared between
/// clones until one of them changes it. A clone costs a reference count; a change
/// copies the list of chunks and the chunk it changes, when another clone shares
/// them.
pub struct CowMap<K, V> {
    /// Never holds an empty chunk.
    chunks: Arc<Vec<Chunk<K, V>>>,
    len: usize,
}

impl<K, V> Clone for CowMap<K, V> {
    fn clone(&self) -> Self {
        Self {
            chunks: Arc::clone(&self.chunks),
            len: self.len,
        }
    }
}

impl<K, V> Default for CowMap<K, V> {
    fn default() -> Self {
        Self {
            chunks: Arc::default(),
            len: 0,
        }
    }
}

impl<K: Ord + Clone, V: Clone> CowMap<K, V> {
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = &(K, V)> {
        entries(self)
    }

    /// The entries from the first whose key `probe` does not place before what it
    /// looks for, in order. `probe` orders each key against what it looks for.
    pub fn seek(&self, probe: impl Fn(&K) -> Ordering) -> impl Iterator<Item = &(K, V)> {
        let (chunk, at) = self.locate(&probe);
        let first = self.chunks.get(chunk).map_or(&[][..], |chunk| &chunk[at..]);
        let rest = self.chunks.get(chunk + 1..).unwrap_or_default();
        first
            .iter()
            .chain(rest.iter().flat_map(|chunk| chunk.iter()))
    }

    /// The entry whose key `probe` orders as equal.
    pub fn get_by(&self, probe: impl Fn(&K) -> Ordering) -> Option<&(K, V)> {
        self.seek(&probe)
            .next()
            .filter(|(key, _)| probe(key) == Ordering::Equal)
    }

    /// Replaces the value under `key`, returning the one it replaced.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let (mut chunk, mut at) = self.locate(&|other: &K| other.cmp(&key));
        let chunks = Arc::make_mut(&mut self.chunks);
        if chunk == chunks.len() {
            let Some(last) = chunks.last() else {
                chunks.push(Arc::new(vec![(key, value)]));
                self.len += 1;
                return None;
            };
            (chunk, at) = (chunks.len() - 1, last.len());
        }
        let entries = Arc::make_mut(&mut chunks[chunk]);
        if let Some((found, old)) = entries.get_mut(at) {
            if *found == key {
                return Some(std::mem::replace(old, value));
            }
        }
        entries.insert(at, (key, value));
        self.len += 1;
        if entries.len() > CHUNK {
            let half = entries.split_off(entries.len() / 2);
            chunks.insert(chunk + 1, Arc::new(half));
        }
        None
    }

    /// Removes the entry under `key`, returning its value.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let (chunk, at) = self.locate(&|other: &K| other.cmp(key));
        let found = self.chunks.get(chunk)?.get(at)?;
        if found.0 != *key {
            return None;
        }
        let chunks = Arc::make_mut(&mut self.chunks);
        let (_, value) = Arc::make_mut(&mut chunks[chunk]).remove(at);
        self.len -= 1;
        let left = chunks[chunk].len();
        if left == 0 {
            chunks.remove(chunk);
        } else if left < CHUNK / 4 {
            join_small(chunks, chunk);
        }
        Some(value)
    }

    /// The chunk and position of the first entry `probe` does not place before
    /// what it looks for; the number of chunks when there is none.
    fn locate(&self, probe: &impl Fn(&K) -> Ordering) -> (usize, usize) {
        let before = |key: &K| probe(key) == Ordering::Less;
        let chunk = self.chunks.partition_point(|chunk| match chunk.last() {
            Some((last, _)) => before(last),
            None => unreachable!("no chunk is empty"),
        });
        match self.chunks.get(chunk) {
            Some(entries) => (chunk, entries.partition_point(|(key, _)| before(key))),
            None => (chunk, 0),
        }
    }
}

/// Repeated keys keep the last value.
impl<K: Ord, V> FromIterator<(K, V)> for CowMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(entries: I) -> Self {
        let mut entries: Vec<(K, V)> = entries.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|later, kept| {
            let repeated = later.0 == kept.0;
            if repeated {
                std::mem::swap(later, kept);
            }
            repeated
        });
        Self::from_sorted(entries)
    }
}

impl<K: Ord, V> CowMap<K, V> {
    /// The map of `entries`, which must be in order of key and name each key once.
    pub fn from_sorted(mut entries: Vec<(K, V)>) -> Self {
        debug_assert!(entries.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let len = entries.len();
        let mut chunks = Vec::with_capacity(len.div_ceil(CHUNK));
        while !entries.is_empty() {
            let last = (entries.len() - 1) / CHUNK * CHUNK;
            chunks.push(Arc::new(entries.split_off(last)));
        }
        chunks.reverse();
        Self {
            chunks: Arc::new(chunks),
            len,
        }
    }
}

/// Joins the small chunk at `at` with a neighbor when both fit in one.
fn join_small<K: Clone, V: Clone>(chunks: &mut Vec<Chunk<K, V>>, at: usize) {
    let fits = |other: usize| chunks[other].len() + chunks[at].len() <= CHUNK;
    let pair = match (at.checked_sub(1), at + 1 < chunks.len()) {
        (Some(before), _) if fits(before) => before,
        (_, true) if fits(at + 1) => at,
        _ => return,
    };
    let next = chunks.remove(pair + 1);
    let joined = Arc::make_mut(&mut chunks[pair]);
    joined.extend(next.iter().cloned());
}

fn entries<K, V>(map: &CowMap<K, V>) -> impl Iterator<Item = &(K, V)> {
    map.chunks.iter().flat_map(|chunk| chunk.iter())
}

impl<K: PartialEq, V: PartialEq> PartialEq for CowMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && entries(self).eq(entries(other))
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for CowMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pairs = entries(self).map(|(key, value)| (key, value));
        f.debug_map().entries(pairs).finish()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// A small deterministic generator.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    fn assert_same(map: &CowMap<u64, u64>, model: &BTreeMap<u64, u64>, step: usize) {
        let entries: Vec<(u64, u64)> = map.iter().copied().collect();
        let expected: Vec<(u64, u64)> = model.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(entries, expected, "step {step}");
        assert_eq!(map.len, model.len(), "step {step}");
        assert!(
            map.chunks
                .iter()
                .all(|chunk| !chunk.is_empty() && chunk.len() <= CHUNK),
            "step {step}: chunk sizes"
        );
    }

    #[test]
    fn random_inserts_and_removes_match_a_btree_map_and_leave_clones_alone() {
        for seed in 1..=20u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            let span = [50, 400, 3000][seed as usize % 3];
            let (mut map, mut model) = (CowMap::default(), BTreeMap::new());
            let mut kept: Vec<(CowMap<u64, u64>, BTreeMap<u64, u64>)> = Vec::new();
            for step in 0..4000 {
                let key = rng.below(span);
                match rng.below(5) {
                    0 | 1 => assert_eq!(map.remove(&key), model.remove(&key), "step {step}"),
                    _ => assert_eq!(
                        map.insert(key, step as u64),
                        model.insert(key, step as u64),
                        "step {step}"
                    ),
                }
                let probe = rng.below(span);
                assert_eq!(
                    map.get_by(|k| k.cmp(&probe)).map(|(_, v)| v),
                    model.get(&probe),
                    "step {step}"
                );
                let from: Vec<u64> = map.seek(|k| k.cmp(&probe)).map(|(k, _)| *k).collect();
                let expected: Vec<u64> = model.range(probe..).map(|(k, _)| *k).collect();
                assert_eq!(from, expected, "step {step}: from {probe}");
                if step % 500 == 0 {
                    kept.push((map.clone(), model.clone()));
                }
            }
            assert_same(&map, &model, 4000);
            for (step, (map, model)) in kept.iter().enumerate() {
                assert_same(map, model, step * 500);
            }
        }
    }

    #[test]
    fn a_collected_map_holds_the_last_value_of_each_key() {
        let entries: Vec<(u64, u64)> = (0..1000).rev().map(|k| (k * 2, k)).collect();
        let mut map: CowMap<u64, u64> = entries.iter().copied().chain([(0, 9)]).collect();
        let mut model: BTreeMap<u64, u64> = entries.into_iter().collect();
        model.insert(0, 9);
        assert_same(&map, &model, 0);
        assert_eq!(map.insert(7, 7), None);
        assert_eq!(map.get_by(|k| k.cmp(&7)), Some(&(7, 7)));
        assert_eq!(map.get_by(|k| k.cmp(&8)), Some(&(8, 4)));
    }
}
