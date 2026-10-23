//! A bounded slice of the value-level checks of design R; `retention_sweep`
//! runs the rest. See `retention/mod.rs`.

mod retention;

use retention::check::{sweep, Depth};
use retention::history::{Act, Bounds};
use retention::spec::Spec;
use retention::world::Val;

const DEPTH: Depth = Depth {
    orders: 24,
    faults: true,
    subsets: true,
};

fn run(bounds: Bounds) {
    let tally = sweep(bounds, DEPTH, Spec::default());
    eprintln!("{}", tally.report());
}

const REGISTERS: Bounds = Bounds {
    writers: 2,
    clone: true,
    entries: 3,
    acts: &[Act::Set(Val::A), Act::Set(Val::B), Act::Clear],
};

#[test]
fn registers_with_a_clone_match_the_oracle() {
    run(REGISTERS);
}

#[test]
fn set_members_with_a_clone_match_the_oracle() {
    run(Bounds {
        writers: 2,
        clone: true,
        entries: 3,
        acts: &[Act::Add(0), Act::Remove(0)],
    });
}

#[test]
fn deletes_and_revives_match_the_oracle() {
    run(Bounds {
        writers: 2,
        clone: false,
        entries: 3,
        acts: &[Act::Set(Val::A), Act::Delete, Act::Revive],
    });
}

#[test]
fn dismissals_match_the_oracle() {
    run(Bounds {
        writers: 2,
        clone: true,
        entries: 3,
        acts: &[Act::Set(Val::A), Act::Set(Val::B), Act::Dismiss],
    });
}

const FOUR: Bounds = Bounds {
    writers: 2,
    clone: true,
    entries: 4,
    acts: &[Act::Set(Val::A), Act::Set(Val::B)],
};

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
        FOUR,
        Spec {
            branch_references: false,
            ..Spec::default()
        },
    );
}

#[test]
fn tips_must_merge_the_union_of_their_pre_fork_writes() {
    let bounds = Bounds {
        acts: &[Act::Set(Val::A), Act::Set(Val::B), Act::Add(0)],
        ..REGISTERS
    };
    must_fail(
        bounds,
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
    let bounds = Bounds {
        writers: 2,
        clone: true,
        entries: 4,
        acts: &[Act::Set(Val::A), Act::Set(Val::B), Act::Dismiss],
    };
    must_fail(
        bounds,
        Spec {
            quarantine: false,
            ..Spec::default()
        },
    );
}

#[test]
#[ignore = "toshokan does not implement design R yet"]
fn toshokan_matches_the_oracle() {
    use retention::{Design, Real};
    let _ = Real.holds(&retention::Reading {
        world: &Default::default(),
        files: &[],
        seen: &[],
    });
}
