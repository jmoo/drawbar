//! A bounded slice of the value-level checks of toshokan under the retention
//! design; `retention_sweep` runs the rest. See `retention/mod.rs`.

mod retention;

use retention::check::{replay, sweep, Depth};
use retention::history::{Act, Bounds};
use retention::world::Val::{A, B};

const DEPTH: Depth = Depth {
    orders: 24,
    faults: true,
    subsets: true,
};

fn run(bounds: Bounds) {
    let tally = sweep(bounds, DEPTH);
    eprintln!("{}", tally.report());
}

#[test]
fn registers_with_a_clone_match_the_oracle() {
    run(Bounds::new(
        2,
        true,
        3,
        &[Act::Set(0, 0, A), Act::Set(0, 0, B), Act::Clear(0, 0)],
    ));
}

#[test]
fn set_members_with_a_clone_match_the_oracle() {
    run(Bounds::new(
        2,
        true,
        3,
        &[Act::Add(0, 0), Act::Remove(0, 0)],
    ));
}

#[test]
fn deletes_and_revives_match_the_oracle() {
    run(Bounds::new(
        2,
        false,
        3,
        &[Act::Set(0, 0, A), Act::Delete(0), Act::Revive(0)],
    ));
}

#[test]
fn dismissals_match_the_oracle() {
    run(Bounds::new(
        2,
        true,
        3,
        &[Act::Set(0, 0, A), Act::Set(0, 0, B), Act::Dismiss],
    ));
}

#[test]
fn copies_resumed_mid_segment_match_the_oracle() {
    run(Bounds {
        shared: true,
        drops: true,
        ..Bounds::new(2, true, 3, &[Act::Set(0, 0, A), Act::Set(0, 0, B)])
    });
}

#[test]
fn two_entities_match_the_oracle() {
    run(Bounds::new(
        2,
        false,
        3,
        &[
            Act::Set(0, 0, A),
            Act::Revive(1),
            Act::Set(1, 0, B),
            Act::Delete(0),
        ],
    ));
}

/// A restore that takes a dismissal and the branch it dismissed takes the fork
/// point with them.
#[test]
fn a_restore_that_takes_a_dismissed_branch_takes_its_fork_point() {
    let acts = [
        "x creates",
        "y Set(0, 0, B)",
        "y is cloned",
        "y' Delete(0), compacts",
        "y Set(0, 0, A)",
        "x after reading Dismiss",
    ];
    let bounds = Bounds::new(
        2,
        true,
        4,
        &[
            Act::Set(0, 0, A),
            Act::Set(0, 0, B),
            Act::Delete(0),
            Act::Dismiss,
        ],
    );
    replay(bounds, DEPTH, &acts);
}

/// A dropped version that leaves a hole below a clone's line: the line still
/// says its chain replaced what came before it, whether a reference names the
/// entry the hole took or the line.
#[test]
fn a_hole_a_dropped_version_left_below_a_clones_line_is_read_as_its_chain_says() {
    let bounds = Bounds::new(
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
    let reads_before_the_clone = [
        "x creates",
        "y Set(0, 0, B)",
        "y Set(0, 0, A)",
        "x after reading Set(0, 0, A)",
        "y is cloned",
        "y' Set(0, 0, A)",
    ];
    let reads_the_clone = [
        "x creates",
        "y Set(0, 0, B)",
        "y Set(0, 0, A)",
        "y is cloned",
        "y' Set(0, 0, A)",
        "x after reading Set(0, 0, A)",
    ];
    for acts in [reads_before_the_clone, reads_the_clone] {
        replay(bounds, DEPTH, &acts);
    }
}

/// A writer that replaced a branch's write before it saw the fork, then cleared
/// the pre-fork write that showed beside its value, keeps both references when
/// it compacts: the cleared value does not come back.
#[test]
fn a_reference_made_after_seeing_a_fork_survives_compaction() {
    let acts = [
        "x creates",
        "y Set(0, 0, A)",
        "y is cloned",
        "y' Add(0, 0)",
        "y Set(0, 0, B)",
        "x after reading Set(0, 0, A)",
        "x after reading Clear(0, 0), compacts",
    ];
    let bounds = Bounds::new(
        2,
        true,
        5,
        &[
            Act::Set(0, 0, A),
            Act::Set(0, 0, B),
            Act::Add(0, 0),
            Act::Clear(0, 0),
        ],
    );
    replay(bounds, DEPTH, &acts);
}

/// A branch that wrote on after the user dismissed its compacted tip is a new
/// item to a reader that no longer sees the other branch, never merged.
#[test]
fn an_entry_after_a_dismissed_tip_is_not_merged_where_the_fork_is_not_seen() {
    let acts = [
        "x creates",
        "y Set(0, 0, A)",
        "y is cloned",
        "y' Add(0, 0), compacts",
        "y Set(0, 0, B)",
        "x after reading Dismiss",
        "y' Set(0, 0, B)",
    ];
    let bounds = Bounds::new(
        2,
        true,
        5,
        &[
            Act::Set(0, 0, A),
            Act::Set(0, 0, B),
            Act::Add(0, 0),
            Act::Dismiss,
        ],
    );
    replay(bounds, DEPTH, &acts);
}
