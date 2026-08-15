//! What the emulator is for: device behaviour that changes as the host works on it.
//!
//! None of this is expressible as a replay — every assertion here depends on state the
//! host's own earlier commands put there.

mod common;

use common::*;
use nord_emu::{Dependency, EmuDevice, Object};
use nord_usb::block_on;
use nord_usb::wire::{Location, ObjectClass};
use nord_usb::{op, Error, Session};

fn program(name: &str) -> Object {
    Object::new(name, b"ne5p", 4, vec![0; 121])
}

/// A walk over a class the host has never seen, across banks and gaps.
///
/// The cursor never leaves its bank, so the walk has to drive it bank by bank; a gap is
/// skipped rather than ending anything; and an empty first slot is not the end of a bank.
#[test]
fn a_cursor_walk_finds_every_object_across_banks_and_gaps() {
    let occupied = [(0, 0), (0, 1), (0, 5), (1, 3)];
    let mut device = EmuDevice::new();
    for (bank, slot) in occupied {
        device.insert(
            ObjectClass::Program,
            Location { bank, slot },
            program(&format!("{bank}:{slot}")),
        );
    }

    let mut t = transport(device);
    let found = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let found = op::occupied_slots(&mut s, 500).await.unwrap();
        s.commit().await.unwrap();
        found
    });

    let found: Vec<(u32, u32)> = found.iter().map(|l| (l.bank, l.slot)).collect();
    assert_eq!(found, occupied);
}

/// The write path's precondition, and the composition it forces.
///
/// ⚠️ The device refuses to overwrite in place, so replacing a slot is delete-then-write
/// — and this is the sequence no capture can hold, because NSM greys the button out
/// rather than sending it.
#[test]
fn a_write_into_an_occupied_slot_is_refused_until_it_is_deleted() {
    let at = Location::from_user(7, 4);
    let mut device = EmuDevice::new();
    device.insert(ObjectClass::Program, at, program("occupant"));
    let file = nord_usb::envelope::wrap("ne5p", at, 4, &[9u8; 121]).unwrap();

    let mut t = transport(device);
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();

        let refused = op::write_program(&mut s, at, &file, 0)
            .await
            .expect_err("the destination was occupied");
        assert!(
            matches!(refused, Error::DeviceStatus(4)),
            "wrong error: {refused}"
        );

        // A refusal is not a desync: the session is still usable, which is what makes
        // the delete-then-write composition possible inside one transaction.
        op::delete(&mut s, at).await.unwrap();
        op::write_program(&mut s, at, &file, 0).await.unwrap();
        op::rename(&mut s, at, "replaced").await.unwrap();
        s.commit().await.unwrap();
    });

    let stored = t.device().get(ObjectClass::Program, at).unwrap();
    assert_eq!(stored.name, "replaced");
    assert_eq!(stored.body, vec![9u8; 121]);
}

/// A body larger than one `READ` comes back in chunks, reassembled in order.
#[test]
fn a_large_body_round_trips_through_the_chunked_read() {
    const CHUNK: usize = 32720;
    let at = Location::from_user(1, 1);
    // Position-dependent, so a chunk reassembled out of order or with a gap fails.
    let body: Vec<u8> = (0..CHUNK * 2 + 777).map(|i| (i % 251) as u8).collect();

    let mut device = EmuDevice::new();
    device.insert(
        ObjectClass::Program,
        at,
        Object::new("big", b"ne5p", 4, body.clone()),
    );

    let mut t = transport(device);
    let got = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        let got = op::read_body(&mut s, at).await.unwrap();
        s.commit().await.unwrap();
        got
    });
    assert_eq!(got, body);

    // Three READs, at the offsets the host chose, with a short final chunk.
    let reads: Vec<&[u8]> = t
        .sent()
        .into_iter()
        .filter(|f| f.get(12..16) == Some(&[0, 0, 0, 0x12]))
        .collect();
    assert_eq!(reads.len(), 3);
    let ranges: Vec<(u32, u32)> = reads
        .iter()
        .map(|f| {
            let w = |i: usize| u32::from_be_bytes(f[i..i + 4].try_into().unwrap());
            (w(24), w(28))
        })
        .collect();
    assert_eq!(
        ranges,
        [
            (0, CHUNK as u32),
            (CHUNK as u32, CHUNK as u32),
            (CHUNK as u32 * 2, 777)
        ]
    );
}

/// An empty slot and an address the class does not have are different refusals, and the
/// difference is what ends a walk.
#[test]
fn empty_and_out_of_range_are_distinguishable() {
    let mut t = transport(EmuDevice::new());
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();

        let empty = op::info(&mut s, Location::from_user(1, 1)).await;
        assert!(matches!(empty, Err(Error::DeviceStatus(1))));

        // The Electro 5's program class is 8 banks of 50.
        let bank = op::info(&mut s, Location::from_user(9, 1)).await;
        assert!(matches!(bank, Err(Error::DeviceStatus(3))));
        let slot = op::info(&mut s, Location::from_user(8, 51)).await;
        assert!(matches!(slot, Err(Error::DeviceStatus(3))));

        // And the geometry is reachable before anything is attempted, which is the
        // point of asking the device rather than assuming.
        let why = op::check_address(&mut s, Location::from_user(9, 1))
            .await
            .unwrap();
        assert!(why.unwrap().contains("this class has 8"));
        assert_eq!(
            op::check_address(&mut s, Location::from_user(8, 50))
                .await
                .unwrap(),
            None
        );
        s.commit().await.unwrap();
    });
}

/// Focus is class-dependent, and two of the three answers are refusals.
#[test]
fn focus_answers_per_class() {
    let at = Location::from_user(5, 1);
    let mut device = EmuDevice::new();
    device.insert(ObjectClass::Program, at, program("loaded"));
    let mut t = transport(device);

    block_on(async {
        // Nothing loaded yet: the panel is not in this class.
        let mut s = Session::open(&mut t, ObjectClass::SetList).await.unwrap();
        assert!(matches!(
            op::focus(&mut s).await,
            Err(Error::DeviceStatus(0x1))
        ));
        s.commit().await.unwrap();

        // A library class has no focus at all — a property of the class, not state.
        let mut s = Session::open(&mut t, ObjectClass::Sample).await.unwrap();
        assert!(matches!(
            op::focus(&mut s).await,
            Err(Error::DeviceStatus(0x15))
        ));
        s.commit().await.unwrap();

        // Selecting an object is what gives the class a focus to report.
        let mut s = Session::open(&mut t, ObjectClass::Program).await.unwrap();
        op::select(&mut s, at).await.unwrap();
        assert_eq!(op::focus(&mut s).await.unwrap(), at);
        s.commit().await.unwrap();
    });
}

/// The `missing` word in a dependency row is resolved at read time.
///
/// A delete performs no fix-up, so the set list keeps pointing at the slot and the row
/// starts reading as dangling — one byte of the reply, changing back when the program is
/// restored. The host's decoder drops that word, so it is read off the frame here.
#[test]
fn deleting_a_referenced_program_dangles_the_reference_without_touching_the_set_list() {
    let song = Location::from_user(1, 43);
    let track = Location::from_user(1, 7);
    let mut device = EmuDevice::new();
    device.insert(ObjectClass::Program, track, program("Bright Grand"));
    device.insert(
        ObjectClass::SetList,
        song,
        Object::new("Jazz Song", b"ne5t", 1, vec![0; 18])
            .with_dependencies(vec![Dependency::slot(ObjectClass::Program, track)]),
    );
    let before = device.get(ObjectClass::SetList, song).cloned();

    let mut t = transport(device);
    block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::SetList).await.unwrap();
        op::dependencies(&mut s, song).await.unwrap();
        s.commit().await.unwrap();

        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        op::delete(&mut s, track).await.unwrap();
        s.commit().await.unwrap();

        let mut s = Session::open(&mut t, ObjectClass::SetList).await.unwrap();
        let rows = op::dependencies(&mut s, song).await.unwrap();
        s.commit().await.unwrap();
        // Routed-ness and presence are two separate axes: the flag does not move.
        assert!(rows[0].is_required());
    });

    let replies: Vec<Vec<u32>> = t
        .received()
        .into_iter()
        .filter(|f| f.get(12..16) == Some(&[0, 0, 0, 0x29]))
        .map(missing_words)
        .collect();
    assert_eq!(replies, [vec![0], vec![1]]);
    assert_eq!(
        t.device().get(ObjectClass::SetList, song).cloned(),
        before,
        "the set list itself must be untouched — the marker is resolved, not stored"
    );
}

/// A move rewrites every reference to the moved object, in every class.
///
/// The instrument maintains referential integrity itself, so a move cannot leave a set
/// list pointing at nothing — which also means a move is not a local operation.
#[test]
fn a_move_rewrites_the_set_lists_that_point_at_the_program() {
    let song = Location::from_user(1, 43);
    let (from, to) = (Location::from_user(1, 7), Location::from_user(7, 10));
    let mut device = EmuDevice::new();
    device.insert(ObjectClass::Program, from, program("Bright Grand"));
    device.insert(
        ObjectClass::SetList,
        song,
        Object::new("Jazz Song", b"ne5t", 1, vec![0; 18])
            .with_dependencies(vec![Dependency::slot(ObjectClass::Program, from)]),
    );

    let mut t = transport(device);
    let rows = block_on(async {
        let mut s = Session::open(&mut t, ObjectClass::Program)
            .await
            .unwrap()
            .allow_destructive_writes();
        op::move_object(&mut s, from, to).await.unwrap();
        s.commit().await.unwrap();

        let mut s = Session::open(&mut t, ObjectClass::SetList).await.unwrap();
        let rows = op::required_dependencies(&mut s, song).await.unwrap();
        s.commit().await.unwrap();
        rows
    });

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].location,
        Some(to),
        "the reference was not rewritten"
    );
}

/// The `missing` word of every row in a `DEPENDENCIES` reply.
///
/// `nord_usb::wire::Dependency` drops it at decode time, so the frame is read here:
/// status, bank, slot, count, then `[u8 flag][u32 missing]…` per row.
fn missing_words(frame: &[u8]) -> Vec<u32> {
    let p = &frame[16 + 4..frame.len() - 2];
    let word = |i: usize| u32::from_be_bytes(p[i..i + 4].try_into().unwrap());
    let count = word(8) as usize;
    let mut out = Vec::new();
    let mut i = 12;
    for _ in 0..count {
        out.push(word(i + 1));
        let name_len = word(i + 13) as usize;
        i += 17 + name_len + 12;
    }
    out
}
