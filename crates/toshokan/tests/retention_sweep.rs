//! The full value-level sweep of design R, which takes minutes:
//! `cargo test --release --test retention_sweep -- --ignored`.

mod retention;

use retention::check::{sweep, Depth};
use retention::history::{Act, Bounds};
use retention::spec::Spec;
use retention::world::Val;

const DEPTH: Depth = Depth {
    orders: 720,
    faults: true,
    subsets: true,
};

const ALL: &[Act] = &[
    Act::Set(Val::A),
    Act::Set(Val::B),
    Act::Clear,
    Act::Add(0),
    Act::Remove(0),
    Act::Delete,
    Act::Revive,
    Act::Dismiss,
];

fn run(bounds: Bounds) {
    let tally = sweep(bounds, DEPTH, Spec::default());
    eprintln!("{}", tally.report());
}

#[test]
#[ignore = "minutes; run by name"]
fn every_act_with_a_clone() {
    run(Bounds {
        writers: 2,
        clone: true,
        entries: 4,
        acts: ALL,
    });
}

#[test]
#[ignore = "minutes; run by name"]
fn three_writers_and_a_clone() {
    run(Bounds {
        writers: 3,
        clone: true,
        entries: 4,
        acts: &[
            Act::Set(Val::A),
            Act::Set(Val::B),
            Act::Delete,
            Act::Revive,
            Act::Dismiss,
        ],
    });
}

#[test]
#[ignore = "minutes; run by name"]
fn dismissals_with_a_clone() {
    run(Bounds {
        writers: 2,
        clone: true,
        entries: 4,
        acts: &[Act::Set(Val::A), Act::Set(Val::B), Act::Dismiss],
    });
}
