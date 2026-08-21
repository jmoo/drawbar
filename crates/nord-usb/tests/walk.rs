//! Golden tests for the enumeration walk, driven by recorded exchanges.
//!
//! The walk is the one hardware-verified surface a replay could not cover, because no
//! capture of it existed: `ReplayTransport` needed a script and NSM never performs a
//! bare enumeration. These scripts are `nord`'s own traffic, taken with `--record`
//! against the instrument.
//!
//! The transport is exact-match, so the walk has to re-emit the same requests in the
//! same order it made them on hardware. What that pins down is the **bank-boundary
//! logic** — where a bank ends, when the walk steps to the next one, and when it stops —
//! which differs per class and is the part most likely to break first on another model.
//! All four addressable classes are covered because all four have different geometry.
//!
//! Corpus-gated: the scripts carry slot names, so they live in the private corpus rather
//! than in this repo.
//!
//! ```sh
//! NORD_CORPUS_ROOT=/path/to/nord-corpus \
//!   cargo test -p nord-usb --features corpus
//! ```

#![cfg(all(feature = "replay", feature = "corpus"))]

use std::path::PathBuf;

use nord_usb::op;
use nord_usb::transport::{Direction, ReplayTransport, Step};
use nord_usb::wire::ObjectClass;
use nord_usb::Session;

/// Where the recorded walks live: the Electro 5 tree's USB recordings.
fn walk_dir() -> PathBuf {
    let root: PathBuf = std::env::var_os("NORD_CORPUS_ROOT")
        .map(PathBuf::from)
        .expect("set NORD_CORPUS_ROOT to a nord-corpus checkout for --features corpus");
    root.join("ne5/usb/device/enumeration_walk")
}

/// Parse a `<O|I> <hex>` script. Blank lines and `#` comments are skipped.
fn script(name: &str) -> Vec<Step> {
    let path = walk_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));

    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            let (tag, hex) = line
                .split_once(' ')
                .expect("a script line is '<O|I> <hex>'");
            let direction = match tag {
                "O" => Direction::Out,
                "I" => Direction::In,
                other => panic!("unknown direction {other:?} in {}", path.display()),
            };
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
                .collect();
            Step { direction, bytes }
        })
        .collect()
}

/// Minimal executor — the crate is runtime-agnostic and a replayed exchange never
/// actually pends, so a busy-poll is sufficient and keeps tokio out of the tree.
fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn vtable() -> &'static RawWakerVTable {
        &RawWakerVTable::new(
            |_| RawWaker::new(std::ptr::null(), vtable()),
            |_| {},
            |_| {},
            |_| {},
        )
    }
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), vtable())) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = unsafe { std::pin::Pin::new_unchecked(&mut fut) };
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// The runaway guard the recordings were made under. It is a bound on a walk that
/// fails to advance, not an item count: a walk that stops *at* the count never issues
/// the probe that discovers the bank has no more occupied slots, and would diverge from
/// the script by one request per bank.
const CAP: usize = 1024;

/// Replay one recorded listing and return the slots it found.
///
/// The recordings are of `nord <noun> list`, which is the walk followed by an `info`
/// per slot found, all inside one session — so the replay has to do both to consume the
/// script. The `info` sweep is what pins the walk's *results*: each address it yields is
/// used to address the device, so a walk that invented a slot would ask for one the
/// recording never answered.
fn walk(name: &str, class: ObjectClass) -> Vec<nord_usb::Location> {
    let mut t = ReplayTransport::new(script(name));
    block_on(async {
        let mut s = Session::open(&mut t, class).await.unwrap();
        let found = op::occupied_slots(&mut s, CAP).await.unwrap();
        for at in &found {
            op::info(&mut s, *at).await.unwrap();
        }
        s.commit().await.unwrap();
        found
    })
}

/// Eight banks of 50, 385 occupied — the walk that crosses the most boundaries.
#[test]
fn program_walk_finds_every_occupied_slot() {
    let found = walk("walk-program.script", ObjectClass::Program);
    assert_eq!(found.len(), 385);

    // Every bank the instrument reports as populated is represented, and the walk
    // leaves each one only after it runs out of occupied slots there.
    let banks: Vec<u32> = found.iter().map(|l| l.bank).collect();
    assert_eq!(*banks.first().unwrap(), 0);
    assert_eq!(*banks.last().unwrap(), 7);
    assert!(
        banks.windows(2).all(|w| w[0] <= w[1]),
        "walk went backwards"
    );
}

/// Four banks of 50, sparsely filled — the class where a walk meets empty banks between
/// populated ones rather than only at the end.
#[test]
fn setlist_walk_finds_every_occupied_slot() {
    let found = walk("walk-setlist.script", ObjectClass::SetList);
    assert_eq!(found.len(), 63);
}

/// A single bank of 159: the walk must stop at the bank's end, not at a bank count.
#[test]
fn sample_walk_finds_every_occupied_slot() {
    let found = walk("walk-sample.script", ObjectClass::Sample);
    assert_eq!(found.len(), 138);
    assert!(found.iter().all(|l| l.bank == 0), "samples are one bank");
}

/// Six banks of 120 — the library class whose banks are named categories rather than
/// numbered slots.
#[test]
fn piano_walk_finds_every_occupied_slot() {
    let found = walk("walk-piano.script", ObjectClass::Piano);
    assert_eq!(found.len(), 29);
}
