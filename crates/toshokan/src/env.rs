//! What the core takes from outside: time, randomness, the app's identity function
//! and the writer's label. Tests inject deterministic ones, so every run replays.

use std::cell::Cell;
use std::rc::Rc;

use crate::ids::{EntityId, Identity, Nonce, SegmentName, WriterId};
use crate::io::Range;

pub trait Clock {
    /// Wall time in milliseconds since the Unix epoch. May go backward; the hybrid
    /// logical clock keeps entries ordered regardless.
    fn now_ms(&mut self) -> u64;
}

pub trait Random {
    /// 128 bits no other writer, instance or clone will draw.
    fn next_u128(&mut self) -> u128;
}

/// The app's cheap identity of a file's contents: enough of the bytes to tell
/// files apart without reading them whole.
pub trait Identify {
    /// The ranges of a file of `len` bytes its identity is computed from.
    fn ranges(&self, len: u64) -> Vec<Range>;

    /// The identity of a file of `len` bytes, from the bytes of `ranges(len)` in
    /// order, each cut short only where the file ends.
    fn identify(&self, len: u64, parts: &[Vec<u8>]) -> Identity;
}

/// Everything nondeterministic the core uses.
pub struct Env {
    pub clock: Box<dyn Clock>,
    pub random: Box<dyn Random>,
    pub identify: Box<dyn Identify>,
    /// Shown to other writers beside this writer's entries. The folder may be
    /// shared, so a generic label is the safe default.
    pub label: String,
}

impl Env {
    pub fn now_ms(&mut self) -> u64 {
        self.clock.now_ms()
    }

    pub fn writer_id(&mut self) -> WriterId {
        WriterId::from_u128(self.random.next_u128())
    }

    pub fn entity_id(&mut self) -> EntityId {
        EntityId::from_u128(self.random.next_u128())
    }

    pub fn segment_name(&mut self) -> SegmentName {
        SegmentName::from_u128(self.random.next_u128())
    }

    pub fn nonce(&mut self) -> Nonce {
        Nonce::from_u128(self.random.next_u128())
    }
}

/// The identity of a file as BLAKE3 over its length and its first `prefix` bytes.
#[derive(Clone, Copy, Debug)]
pub struct PrefixIdentity {
    pub prefix: u64,
}

impl Default for PrefixIdentity {
    fn default() -> Self {
        Self { prefix: 64 * 1024 }
    }
}

impl Identify for PrefixIdentity {
    fn ranges(&self, len: u64) -> Vec<Range> {
        vec![Range {
            offset: 0,
            len: len.min(self.prefix),
        }]
    }

    fn identify(&self, len: u64, parts: &[Vec<u8>]) -> Identity {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&len.to_be_bytes());
        parts.iter().for_each(|part| {
            hasher.update(part);
        });
        let mut first = [0; 16];
        first.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
        Identity::from_u128(u128::from_be_bytes(first))
    }
}

/// A clock tests set by hand. Clones share one reading.
#[derive(Clone, Default, Debug)]
pub struct TestClock(Rc<Cell<u64>>);

impl TestClock {
    pub fn at(now_ms: u64) -> Self {
        Self(Rc::new(Cell::new(now_ms)))
    }

    pub fn set(&self, now_ms: u64) {
        self.0.set(now_ms);
    }

    pub fn advance(&self, ms: u64) {
        self.0.set(self.0.get().saturating_add(ms));
    }
}

impl Clock for TestClock {
    fn now_ms(&mut self) -> u64 {
        self.0.get()
    }
}

/// A deterministic generator for tests: SplitMix64 from a seed.
#[derive(Clone, Debug)]
pub struct SeededRandom(u64);

impl SeededRandom {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

impl Random for SeededRandom {
    fn next_u128(&mut self) -> u128 {
        (u128::from(self.next_u64()) << 64) | u128::from(self.next_u64())
    }
}

/// The system's wall clock.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Default, Debug)]
pub struct SystemClock;

#[cfg(not(target_arch = "wasm32"))]
impl Clock for SystemClock {
    fn now_ms(&mut self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            })
    }
}

/// Randomness from the operating system, through the keys the standard library
/// draws for its hash maps, mixed with the time of each draw.
#[cfg(not(target_arch = "wasm32"))]
pub struct OsRandom {
    keys: std::collections::hash_map::RandomState,
    draws: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl OsRandom {
    pub fn new() -> Self {
        Self {
            keys: std::collections::hash_map::RandomState::new(),
            draws: 0,
        }
    }

    fn half(&mut self) -> u64 {
        use std::hash::{BuildHasher, Hasher};
        self.draws += 1;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let mut hasher = self.keys.build_hasher();
        hasher.write_u64(self.draws);
        hasher.write_u128(nanos);
        hasher.finish()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for OsRandom {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Random for OsRandom {
    fn next_u128(&mut self) -> u128 {
        (u128::from(self.half()) << 64) | u128::from(self.half())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_randomness_repeats_per_seed() {
        let draw = |seed| {
            let mut random = SeededRandom::new(seed);
            [random.next_u128(), random.next_u128()]
        };
        assert_eq!(draw(1), draw(1));
        assert_ne!(draw(1), draw(2));
        assert_ne!(draw(1)[0], draw(1)[1]);
    }

    #[test]
    fn a_prefix_identity_reads_at_most_its_prefix_and_counts_the_length() {
        let identity = PrefixIdentity { prefix: 4 };
        assert_eq!(identity.ranges(10), [Range { offset: 0, len: 4 }]);
        assert_eq!(identity.ranges(2), [Range { offset: 0, len: 2 }]);
        let head = vec![b"abcd".to_vec()];
        assert_ne!(identity.identify(10, &head), identity.identify(11, &head));
        assert_ne!(
            identity.identify(10, &head),
            identity.identify(10, &[b"abce".to_vec()])
        );
    }
}
