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
    /// Each chunk with its last key, which a search compares without reading
    /// the chunk. Never holds an empty chunk.
    chunks: Arc<Vec<(K, Chunk<K, V>)>>,
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

impl<K, V> CowMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        entries(self).map(|(key, _)| key)
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

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn iter(&self) -> impl Iterator<Item = &(K, V)> {
        entries(self)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        entries(self).map(|(_, value)| value)
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.get_by(|other| other.cmp(key)).map(|(_, value)| value)
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// The entries after `after`, or every entry, in order.
    pub fn after(&self, after: Option<K>) -> impl Iterator<Item = &(K, V)> {
        let probe = after.clone();
        let start = self.seek(move |key| match &probe {
            Some(after) => key.cmp(after),
            None => Ordering::Greater,
        });
        start.skip_while(move |(key, _)| Some(key) == after.as_ref())
    }

    /// The value under `key`, its chunk copied first when a clone shares it.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let (chunk, at) = self.locate(&|other: &K| other.cmp(key));
        self.held_mut(chunk, at, key)
    }

    /// The value at `at` of chunk `chunk`, when it is under `key`.
    fn held_mut(&mut self, chunk: usize, at: usize, key: &K) -> Option<&mut V> {
        if self.chunks.get(chunk)?.1.get(at)?.0 != *key {
            return None;
        }
        let chunks = Arc::make_mut(&mut self.chunks);
        Some(&mut Arc::make_mut(&mut chunks[chunk].1)[at].1)
    }

    /// The value under `key`, first inserting what `new` makes when there is none.
    pub fn get_or_insert_with(&mut self, key: K, new: impl FnOnce() -> V) -> &mut V {
        let (chunk, at) = self.locate(&|other: &K| other.cmp(&key));
        if self.held_mut(chunk, at, &key).is_none() {
            self.insert(key.clone(), new());
            return self.get_mut(&key).expect("inserted above");
        }
        self.held_mut(chunk, at, &key).expect("held above")
    }

    /// Each key this map or `before` holds, with its entry in each, in order of
    /// key, but for the chunks the two share: those are skipped, so the cost
    /// follows what changed since one was cloned from the other.
    pub fn diff<'a>(&'a self, before: &'a Self) -> Diff<'a, K, V> {
        Diff {
            now: Cursor::new(&self.chunks),
            was: Cursor::new(&before.chunks),
        }
    }

    /// The entries from the first whose key `probe` does not place before what it
    /// looks for, in order. `probe` orders each key against what it looks for.
    pub fn seek(&self, probe: impl Fn(&K) -> Ordering) -> impl Iterator<Item = &(K, V)> {
        let (chunk, at) = self.locate(&probe);
        let first = self
            .chunks
            .get(chunk)
            .map_or(&[][..], |(_, chunk)| &chunk[at..]);
        let rest = self.chunks.get(chunk + 1..).unwrap_or_default();
        first
            .iter()
            .chain(rest.iter().flat_map(|(_, chunk)| chunk.iter()))
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
            let Some((_, last)) = chunks.last() else {
                chunks.push((key.clone(), Arc::new(vec![(key, value)])));
                self.len += 1;
                return None;
            };
            (chunk, at) = (chunks.len() - 1, last.len());
        }
        let final_chunk = chunk + 1 == chunks.len();
        let (last, entries) = &mut chunks[chunk];
        let entries = Arc::make_mut(entries);
        if let Some((found, old)) = entries.get_mut(at) {
            if *found == key {
                return Some(std::mem::replace(old, value));
            }
        }
        if at == entries.len() {
            *last = key.clone();
        }
        entries.insert(at, (key, value));
        self.len += 1;
        if entries.len() > CHUNK {
            // Entries added in order fill each chunk before starting the next.
            let appended = final_chunk && at + 1 == entries.len();
            let from = match appended {
                true => entries.len() - 1,
                false => entries.len() / 2,
            };
            let half = entries.split_off(from);
            entries.shrink_to(CHUNK);
            let left = entries.last().expect("half is not all").0.clone();
            let right = std::mem::replace(last, left);
            chunks.insert(chunk + 1, (right, Arc::new(half)));
        }
        None
    }

    /// Removes the entry under `key`, returning its value.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let (chunk, at) = self.locate(&|other: &K| other.cmp(key));
        let found = self.chunks.get(chunk)?.1.get(at)?;
        if found.0 != *key {
            return None;
        }
        let chunks = Arc::make_mut(&mut self.chunks);
        let (last, entries) = &mut chunks[chunk];
        let entries = Arc::make_mut(entries);
        let (_, value) = entries.remove(at);
        self.len -= 1;
        match entries.last() {
            None => {
                chunks.remove(chunk);
            }
            Some((kept, _)) => {
                *last = kept.clone();
                if entries.len() < CHUNK / 4 {
                    join_small(chunks, chunk);
                }
            }
        }
        Some(value)
    }

    /// The chunk and position of the first entry `probe` does not place before
    /// what it looks for; the number of chunks when there is none.
    fn locate(&self, probe: &impl Fn(&K) -> Ordering) -> (usize, usize) {
        let before = |key: &K| probe(key) == Ordering::Less;
        let chunk = self.chunks.partition_point(|(last, _)| before(last));
        match self.chunks.get(chunk) {
            Some((_, entries)) => (chunk, entries.partition_point(|(key, _)| before(key))),
            None => (chunk, 0),
        }
    }
}

impl<'a, K, V> IntoIterator for &'a CowMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Box<dyn Iterator<Item = (&'a K, &'a V)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(entries(self).map(|(key, value)| (key, value)))
    }
}

impl<K: Ord + Clone, V: Clone> std::ops::Index<&K> for CowMap<K, V> {
    type Output = V;

    fn index(&self, key: &K) -> &V {
        self.get(key).expect("a key the map holds")
    }
}

impl<K: Ord + Clone, V, const N: usize> From<[(K, V); N]> for CowMap<K, V> {
    fn from(entries: [(K, V); N]) -> Self {
        entries.into_iter().collect()
    }
}

/// Repeated keys keep the last value.
impl<K: Ord + Clone, V> FromIterator<(K, V)> for CowMap<K, V> {
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

impl<K: Ord + Clone, V> CowMap<K, V> {
    /// The map of `entries`, which must be in order of key and name each key once.
    pub fn from_sorted(entries: Vec<(K, V)>) -> Self {
        debug_assert!(entries.windows(2).all(|pair| pair[0].0 < pair[1].0));
        Self::from_sorted_iter(entries)
    }

    /// The map of `entries`, which must come in order of key and name each key
    /// once, chunked as they come, without a list of them all.
    pub fn from_sorted_iter(entries: impl IntoIterator<Item = (K, V)>) -> Self {
        let mut chunks = Vec::new();
        let mut chunk = Vec::with_capacity(CHUNK);
        let mut len = 0;
        for entry in entries {
            len += 1;
            chunk.push(entry);
            if chunk.len() == CHUNK {
                let full = std::mem::replace(&mut chunk, Vec::with_capacity(CHUNK));
                let last = full[CHUNK - 1].0.clone();
                chunks.push((last, Arc::new(full)));
            }
        }
        if let Some((last, _)) = chunk.last() {
            let last = last.clone();
            chunk.shrink_to_fit();
            chunks.push((last, Arc::new(chunk)));
        }
        Self {
            chunks: Arc::new(chunks),
            len,
        }
    }
}

/// What [`CowMap::diff`] gives: the entries of each key, now and before.
pub struct Diff<'a, K, V> {
    now: Cursor<'a, K, V>,
    was: Cursor<'a, K, V>,
}

/// A place in a map's chunks.
struct Cursor<'a, K, V> {
    chunks: &'a [(K, Chunk<K, V>)],
    chunk: usize,
    at: usize,
}

impl<'a, K, V> Cursor<'a, K, V> {
    fn new(chunks: &'a [(K, Chunk<K, V>)]) -> Self {
        Self {
            chunks,
            chunk: 0,
            at: 0,
        }
    }

    fn peek(&self) -> Option<&'a (K, V)> {
        self.chunks.get(self.chunk)?.1.get(self.at)
    }

    fn step(&mut self) {
        self.at += 1;
        if self
            .chunks
            .get(self.chunk)
            .is_some_and(|(_, chunk)| self.at == chunk.len())
        {
            self.skip();
        }
    }

    fn skip(&mut self) {
        self.chunk += 1;
        self.at = 0;
    }

    /// The chunk this cursor is at the start of.
    fn starting(&self) -> Option<&'a Chunk<K, V>> {
        let chunk = self.chunks.get(self.chunk).filter(|_| self.at == 0);
        chunk.map(|(_, chunk)| chunk)
    }
}

impl<'a, K: Ord, V> Iterator for Diff<'a, K, V> {
    type Item = (Option<&'a (K, V)>, Option<&'a (K, V)>);

    fn next(&mut self) -> Option<Self::Item> {
        while let (Some(now), Some(was)) = (self.now.starting(), self.was.starting()) {
            if !Arc::ptr_eq(now, was) {
                break;
            }
            self.now.skip();
            self.was.skip();
        }
        let order = match (self.now.peek(), self.was.peek()) {
            (None, None) => return None,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(now), Some(was)) => now.0.cmp(&was.0),
        };
        let now = (order != Ordering::Greater)
            .then(|| self.now.peek())
            .flatten();
        let was = (order != Ordering::Less).then(|| self.was.peek()).flatten();
        if now.is_some() {
            self.now.step();
        }
        if was.is_some() {
            self.was.step();
        }
        Some((now, was))
    }
}

/// Joins the small chunk at `at` with a neighbor when both fit in one.
fn join_small<K: Clone, V: Clone>(chunks: &mut Vec<(K, Chunk<K, V>)>, at: usize) {
    let fits = |other: usize| chunks[other].1.len() + chunks[at].1.len() <= CHUNK;
    let pair = match (at.checked_sub(1), at + 1 < chunks.len()) {
        (Some(before), _) if fits(before) => before,
        (_, true) if fits(at + 1) => at,
        _ => return,
    };
    let (last, next) = chunks.remove(pair + 1);
    let (joined_last, joined) = &mut chunks[pair];
    Arc::make_mut(joined).extend(next.iter().cloned());
    *joined_last = last;
}

fn entries<K, V>(map: &CowMap<K, V>) -> impl Iterator<Item = &(K, V)> {
    map.chunks.iter().flat_map(|(_, chunk)| chunk.iter())
}

impl<K: PartialEq, V: PartialEq> PartialEq for CowMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && entries(self).eq(entries(other))
    }
}

impl<K: Eq, V: Eq> Eq for CowMap<K, V> {}

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
            map.chunks.iter().all(|(last, chunk)| !chunk.is_empty()
                && chunk.len() <= CHUNK
                && chunk.last().map(|(key, _)| key) == Some(last)),
            "step {step}: chunk sizes and last keys"
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
                match rng.below(6) {
                    0 | 1 => assert_eq!(map.remove(&key), model.remove(&key), "step {step}"),
                    2 => {
                        *map.get_or_insert_with(key, || step as u64) += 1;
                        *model.entry(key).or_insert(step as u64) += 1;
                    }
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
                let after: Vec<u64> = map.after(Some(probe)).map(|(k, _)| *k).collect();
                let beyond = (std::ops::Bound::Excluded(probe), std::ops::Bound::Unbounded);
                let expected: Vec<u64> = model.range(beyond).map(|(k, _)| *k).collect();
                assert_eq!(after, expected, "step {step}: after {probe}");
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

    /// Each key of `now` or `was` whose value differs, as both models hold it.
    fn model_diff(
        now: &BTreeMap<u64, u64>,
        was: &BTreeMap<u64, u64>,
    ) -> Vec<(u64, Option<u64>, Option<u64>)> {
        let keys: std::collections::BTreeSet<u64> = now.keys().chain(was.keys()).copied().collect();
        keys.into_iter()
            .map(|key| (key, now.get(&key).copied(), was.get(&key).copied()))
            .filter(|(_, now, was)| now != was)
            .collect()
    }

    #[test]
    fn a_diff_from_a_clone_gives_every_changed_key_and_skips_shared_chunks() {
        for seed in 1..=20u64 {
            let mut rng = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d));
            let span = [100, 2000, 20_000][seed as usize % 3];
            let model: BTreeMap<u64, u64> = (0..span / 2).map(|k| (k * 2, k)).collect();
            let was: CowMap<u64, u64> = model.iter().map(|(k, v)| (*k, *v)).collect();
            let (mut now, mut changed) = (was.clone(), model.clone());
            for step in 0..rng.below(40) {
                let key = rng.below(span);
                match rng.below(3) {
                    0 => assert_eq!(now.remove(&key), changed.remove(&key)),
                    1 => assert_eq!(now.insert(key, step), changed.insert(key, step)),
                    _ => {
                        if let Some(value) = now.get_mut(&key) {
                            *value = step;
                            changed.insert(key, step);
                        }
                    }
                }
            }
            let mut differences = Vec::new();
            let mut compared = 0;
            for (n, w) in now.diff(&was) {
                compared += 1;
                let key = n.or(w).map(|(key, _)| *key).unwrap();
                let (n, w) = (n.map(|(_, v)| *v), w.map(|(_, v)| *v));
                if n != w {
                    differences.push((key, n, w));
                }
            }
            assert_eq!(differences, model_diff(&changed, &model), "seed {seed}");
            let touched = model_diff(&changed, &model).len();
            assert!(
                compared <= (touched + 1) * 4 * CHUNK,
                "seed {seed}: {compared} entries compared for {touched} changes"
            );
            assert_same(&was, &model, 0);
        }
    }

    #[test]
    fn a_map_filled_in_order_fills_each_chunk() {
        let mut map = CowMap::default();
        for key in 0..1000u64 {
            map.insert(key, key);
        }
        let model: BTreeMap<u64, u64> = (0..1000).map(|key| (key, key)).collect();
        assert_same(&map, &model, 1000);
        let sizes: Vec<usize> = map.chunks.iter().map(|(_, chunk)| chunk.len()).collect();
        assert_eq!(sizes, [128, 128, 128, 128, 128, 128, 128, 104]);
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
