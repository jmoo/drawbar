//! A bounded slice of the value-level checks of the retention design; `retention_sweep`
//! runs the rest. See `retention/mod.rs`.

mod retention;

use retention::check::{replay, sweep, Depth};
use retention::history::{Act, Bounds};
use retention::real::Real;
use retention::spec::Spec;
use retention::world::Val::{A, B};

const DEPTH: Depth = Depth {
    orders: 24,
    faults: true,
    subsets: true,
};

fn run(bounds: Bounds) {
    let tally = sweep(bounds, DEPTH, Spec::default());
    eprintln!("{}", tally.report());
}

fn run_real(bounds: Bounds) {
    let tally = sweep(bounds, DEPTH, Real);
    eprintln!("{}", tally.report());
}

const REGISTERS: Bounds = Bounds::new(
    2,
    true,
    3,
    &[Act::Set(0, 0, A), Act::Set(0, 0, B), Act::Clear(0, 0)],
);

#[test]
fn registers_with_a_clone_match_the_oracle() {
    run(REGISTERS);
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
fn a_copy_resumed_mid_segment_matches_the_oracle() {
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

/// Sweeps `bounds` with one of `RETENTION.md`'s rules switched off, which must break
/// a property; with every rule on, the same bounds pass here or in
/// `retention_sweep`.
fn must_fail(bounds: Bounds, design: Spec) {
    let payload = std::panic::catch_unwind(|| sweep(bounds, DEPTH, design))
        .expect_err("the sweep finds no violation");
    let message = match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .unwrap_or_default(),
    };
    eprintln!("{message}");
}

#[test]
fn references_made_on_a_branch_must_not_apply() {
    must_fail(
        Bounds::new(2, true, 4, &[Act::Set(0, 0, A), Act::Set(0, 0, B)]),
        Spec {
            branch_references: false,
            ..Spec::default()
        },
    );
}

#[test]
fn tips_must_merge_the_union_of_their_pre_fork_writes() {
    must_fail(
        Bounds::new(
            2,
            true,
            3,
            &[Act::Set(0, 0, A), Act::Set(0, 0, B), Act::Add(0, 0)],
        ),
        Spec {
            union: false,
            ..Spec::default()
        },
    );
}

#[test]
fn a_newer_own_write_must_kill_an_older_one() {
    must_fail(
        REGISTERS,
        Spec {
            supersede: false,
            ..Spec::default()
        },
    );
}

#[test]
fn dismissed_entries_must_never_be_merged() {
    must_fail(
        Bounds::new(
            2,
            true,
            4,
            &[Act::Set(0, 0, A), Act::Set(0, 0, B), Act::Dismiss],
        ),
        Spec {
            quarantine: false,
            ..Spec::default()
        },
    );
}

#[test]
fn toshokan_registers_with_a_clone_match_the_oracle() {
    run_real(REGISTERS);
}

#[test]
fn toshokan_set_members_with_a_clone_match_the_oracle() {
    run_real(Bounds::new(
        2,
        true,
        3,
        &[Act::Add(0, 0), Act::Remove(0, 0)],
    ));
}

#[test]
fn toshokan_deletes_and_revives_match_the_oracle() {
    run_real(Bounds::new(
        2,
        false,
        3,
        &[Act::Set(0, 0, A), Act::Delete(0), Act::Revive(0)],
    ));
}

#[test]
fn toshokan_dismissals_match_the_oracle() {
    run_real(Bounds::new(
        2,
        true,
        3,
        &[Act::Set(0, 0, A), Act::Set(0, 0, B), Act::Dismiss],
    ));
}

#[test]
fn toshokan_copies_resumed_mid_segment_match_the_oracle() {
    run_real(Bounds {
        shared: true,
        drops: true,
        ..Bounds::new(2, true, 3, &[Act::Set(0, 0, A), Act::Set(0, 0, B)])
    });
}

#[test]
fn toshokan_with_two_entities_matches_the_oracle() {
    run_real(Bounds::new(
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
    replay(bounds, DEPTH, Spec::default(), &acts);
    replay(bounds, DEPTH, Real, &acts);
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
        replay(bounds, DEPTH, Spec::default(), &acts);
        replay(bounds, DEPTH, Real, &acts);
    }
}
