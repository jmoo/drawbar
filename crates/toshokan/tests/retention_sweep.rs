//! The full value-level sweep of design R, which takes minutes:
//! `cargo test --release --test retention_sweep -- --ignored`.

mod retention;

use retention::check::{sweep, Depth};
use retention::history::{Act, Bounds};
use retention::spec::Spec;
use retention::world::Val::{A, B};

const DEPTH: Depth = Depth {
    orders: 720,
    faults: true,
    subsets: true,
};

const ALL: &[Act] = &[
    Act::Set(0, 0, A),
    Act::Set(0, 0, B),
    Act::Clear(0, 0),
    Act::Add(0, 0),
    Act::Remove(0, 0),
    Act::Delete(0),
    Act::Revive(0),
    Act::Dismiss,
    Act::Take,
];

fn run(bounds: Bounds) {
    let tally = sweep(bounds, DEPTH, Spec::default());
    eprintln!("{}", tally.report());
}

#[test]
#[ignore = "minutes; run by name"]
fn every_act_with_a_clone() {
    run(Bounds::new(2, true, 4, ALL));
}

#[test]
#[ignore = "minutes; run by name"]
fn three_writers_and_a_clone() {
    run(Bounds::new(
        3,
        true,
        4,
        &[
            Act::Set(0, 0, A),
            Act::Set(0, 0, B),
            Act::Delete(0),
            Act::Revive(0),
            Act::Dismiss,
        ],
    ));
}

#[test]
#[ignore = "minutes; run by name"]
fn dismissals_and_takes_with_a_clone() {
    run(Bounds::new(
        2,
        true,
        4,
        &[
            Act::Set(0, 0, A),
            Act::Set(0, 0, B),
            Act::Dismiss,
            Act::Take,
        ],
    ));
}

#[test]
#[ignore = "minutes; run by name"]
fn copies_resumed_mid_segment_and_dropped_versions() {
    run(Bounds {
        shared: true,
        drops: true,
        ..Bounds::new(
            2,
            true,
            4,
            &[
                Act::Set(0, 0, A),
                Act::Set(0, 0, B),
                Act::Delete(0),
                Act::Revive(0),
            ],
        )
    });
}

#[test]
#[ignore = "minutes; run by name"]
fn two_registers_two_members_and_two_entities() {
    run(Bounds::new(
        2,
        true,
        4,
        &[
            Act::Set(0, 0, A),
            Act::Set(0, 1, B),
            Act::Add(0, 1),
            Act::Delete(0),
            Act::Revive(1),
            Act::Set(1, 0, A),
        ],
    ));
}
