//! A bounded slice of the value-level checks of design R; `retention_sweep`
//! runs the rest. See `retention/mod.rs`.

mod retention;

use retention::check::{sweep, Depth};
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

/// Sweeps `bounds` with one of design R's rules switched off, which must break
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
