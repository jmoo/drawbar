//! The full value-level sweep of the retention design, of its rules and of
//! toshokan: `cargo test --profile release-tests --test retention_sweep -- --ignored`.

mod retention;

use retention::check::{sweep, Depth};
use retention::history::{Act, Bounds};
use retention::real::Real;
use retention::spec::Spec;
use retention::world::Val::{A, B};
use retention::Design;

const DEPTH: Depth = Depth {
    orders: 720,
    faults: true,
    subsets: true,
};

const EVERY_ACT: Bounds = Bounds::new(
    2,
    true,
    4,
    &[
        Act::Set(0, 0, A),
        Act::Set(0, 0, B),
        Act::Clear(0, 0),
        Act::Add(0, 0),
        Act::Remove(0, 0),
        Act::Delete(0),
        Act::Revive(0),
        Act::Dismiss,
        Act::Take,
    ],
);

const THREE_WRITERS: Bounds = Bounds::new(
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
);

const TAKES: Bounds = Bounds::new(
    2,
    true,
    4,
    &[
        Act::Set(0, 0, A),
        Act::Set(0, 0, B),
        Act::Dismiss,
        Act::Take,
    ],
);

const COPIES: Bounds = Bounds {
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
};

const TWO_ENTITIES: Bounds = Bounds::new(
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
);

fn run<D: Design + Copy + Sync>(bounds: Bounds, design: D) {
    let tally = sweep(bounds, DEPTH, design);
    eprintln!("{}", tally.report());
}

#[test]
#[ignore = "minutes; run by name"]
fn every_act_with_a_clone() {
    run(EVERY_ACT, Spec::default());
}

#[test]
#[ignore = "minutes; run by name"]
fn three_writers_and_a_clone() {
    run(THREE_WRITERS, Spec::default());
}

#[test]
#[ignore = "minutes; run by name"]
fn dismissals_and_takes_with_a_clone() {
    run(TAKES, Spec::default());
}

#[test]
#[ignore = "minutes; run by name"]
fn copies_resumed_mid_segment_and_dropped_versions() {
    run(COPIES, Spec::default());
}

#[test]
#[ignore = "minutes; run by name"]
fn two_registers_two_members_and_two_entities() {
    run(TWO_ENTITIES, Spec::default());
}

#[test]
#[ignore = "minutes; run by name"]
fn toshokan_every_act_with_a_clone() {
    run(EVERY_ACT, Real);
}

#[test]
#[ignore = "minutes; run by name"]
fn toshokan_three_writers_and_a_clone() {
    run(THREE_WRITERS, Real);
}

#[test]
#[ignore = "minutes; run by name"]
fn toshokan_dismissals_and_takes_with_a_clone() {
    run(TAKES, Real);
}

#[test]
#[ignore = "minutes; run by name"]
fn toshokan_copies_resumed_mid_segment_and_dropped_versions() {
    run(COPIES, Real);
}

#[test]
#[ignore = "minutes; run by name"]
fn toshokan_two_registers_two_members_and_two_entities() {
    run(TWO_ENTITIES, Real);
}
