//! Recordings of `nord <noun> list` from the private corpus, gated on `corpus` because
//! the scripts carry slot names.
//!
//! Each test states the slot count its walk must find; `tests/replay` drives the same
//! files for their bytes.

#![cfg(all(feature = "replay", feature = "corpus"))]

#[path = "support/geometry.rs"]
mod geometry;
#[path = "support/scripts.rs"]
mod scripts;

use nord_usb::op;
use nord_usb::transport::{ReplayTransport, Step};
use nord_usb::wire::{Bank, ObjectClass};
use nord_usb::Session;

/// One recorded walk, by its name under the Electro 5 tree's USB recordings.
fn script(name: &str) -> Vec<Step> {
    scripts::read(
        &scripts::corpus()
            .join("ne5/usb/device/enumeration_walk")
            .join(name),
    )
    .steps()
}

/// The banks each walk is bounded by. The recordings carry no geometry section of their
/// own, so they are bounded by the committed one.
fn banks(class: ObjectClass) -> Vec<Bank> {
    pollster::block_on(geometry::committed())
        .expect("the committed geometry recording")
        .banks(class)
        .unwrap_or_else(|e| panic!("{}: {e}", class.label()))
        .to_vec()
}

/// Replay one recorded listing and return the slots it found.
///
/// The recordings are of `nord <noun> list`, which is the walk followed by an `info`
/// per slot found, all inside one session, so the replay does both to consume the
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

/// Eight banks of 50, 385 occupied: the walk that crosses the most boundaries.
#[test]
fn program_walk_finds_every_occupied_slot() {
    let found = walk("walk-program.script", ObjectClass::Program);
    assert_eq!(found.len(), 385);

    let banks: Vec<u32> = found.iter().map(|l| l.bank).collect();
    assert_eq!(*banks.first().unwrap(), 0);
    assert_eq!(*banks.last().unwrap(), 7);
    assert!(banks.windows(2).all(|w| w[0] <= w[1]), "walk went backward");
}

/// Four banks of 50, sparsely filled, so the walk meets empty banks between populated
/// ones and not only at the end.
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

/// Six banks of 20, named by category.
#[test]
fn piano_walk_finds_every_occupied_slot() {
    let found = walk("walk-piano.script", ObjectClass::Piano);
    assert_eq!(found.len(), 29);
}
