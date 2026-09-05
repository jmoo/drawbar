//! Enumeration walks driven by recorded exchanges.
//!
//! The walk is the one hardware-verified surface a replay could not cover, because no
//! capture of it existed: `ReplayTransport` needed a script and NSM never performs a
//! bare enumeration. These scripts are `nord`'s own traffic, taken with `--record`
//! against the instrument.
//!
//! The exact-match transport checks bank boundaries, transitions, and termination for
//! each addressable class.
//!
//! Corpus-gated: the scripts carry slot names, so they live in the private corpus rather
//! than in this repo.
//!
//! What is here rather than in `tests/replay` is the *result* of each walk: the sweep
//! drives a script and checks its bytes, and these four also say how many slots the walk
//! must find. Give those scripts a `# intent: <class> walk` header and the sweep drives
//! them too, from the same files.
//!
//! ```sh
//! NORD_CORPUS_ROOT=/path/to/nord-corpus \
//!   cargo test -p nord-usb --features corpus
//! ```

#![cfg(all(feature = "replay", feature = "corpus"))]

use std::path::PathBuf;

#[path = "support/scripts.rs"]
mod scripts;

use nord_usb::device::Geometry;
use nord_usb::op;
use nord_usb::transport::{Direction, ReplayTransport, Step};
use nord_usb::wire::{Bank, ObjectClass};
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

/// The banks each walk is bounded by. The recordings carry no geometry section of their
/// own, so they are read from the committed recording of `device geometry` — the same
/// instrument, and tables that do not change.
fn banks(class: ObjectClass) -> Vec<Bank> {
    let mut t = ReplayTransport::new(scripts::fixture("device/geometry.script").steps());
    pollster::block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let geometry = Geometry::read(&mut s).await.unwrap();
        s.commit().await.unwrap();
        geometry.banks(class).unwrap().to_vec()
    })
}

/// Replay one recorded listing and return the slots it found.
///
/// The recordings are of `nord <noun> list`, which is the walk followed by an `info`
/// per slot found, all inside one session — so the replay has to do both to consume the
/// script. Reading `info` for every result makes an invented address fail the replay.
fn walk(name: &str, class: ObjectClass) -> Vec<nord_usb::Location> {
    let banks = banks(class);
    let mut t = ReplayTransport::new(script(name));
    pollster::block_on(async {
        let mut s = Session::open(&mut t, class).await.unwrap();
        let found = op::occupied_slots(&mut s, &banks).await.unwrap();
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

/// Six banks of 20 — the library class whose banks are named categories rather than
/// numbered slots.
#[test]
fn piano_walk_finds_every_occupied_slot() {
    let found = walk("walk-piano.script", ObjectClass::Piano);
    assert_eq!(found.len(), 29);
}
