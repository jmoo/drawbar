//! Every state reads as its oracle says: opened with its local root, and as a fresh
//! install whose local root is empty. Opening and a scan of every file write
//! nothing in the folder.

use serde_json::Value;
use toshokan::env::TestClock;
use toshokan::simulator::Machine;
use toshokan::Root;

use crate::fixture::{fixtures, load, load_folder, open, Faulty, STATES};
use crate::oracle::{differences, observed, oracle};

/// What a reader on `machine` shows once it has scanned every file.
pub fn read(machine: &Machine) -> Value {
    let before = machine.folder.files(Root::Folder);
    let backend = Faulty::new(machine);
    let (mut library, opened) = open(&backend, "reader", 99, &TestClock::at(100_000));
    library.rescan().unwrap();
    let found = observed(&library, &opened, &machine.folder);
    assert_eq!(
        machine.folder.files(Root::Folder),
        before,
        "opening and scanning wrote in the folder"
    );
    found
}

/// Each fact on which state `state` reads otherwise than its oracle says, opened
/// with its local root and on a fresh install.
fn read_otherwise(state: &str) -> Vec<String> {
    let expected = oracle(state, "view.oracle.json");
    let mut found = differences(state, &expected, &read(&load(state)));

    let mut fresh = expected;
    fresh["start"] = "new".into();
    fresh["mode"] = "writable".into();
    let at = format!("{state} on a fresh install");
    found.extend(differences(&at, &fresh, &read(&load_folder(state))));
    found
}

#[test]
fn every_state_reads_as_its_oracle() {
    let found: Vec<String> = STATES
        .iter()
        .flat_map(|state| read_otherwise(state))
        .collect();
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn every_state_is_read() {
    let mut found: Vec<String> = std::fs::read_dir(fixtures())
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .map(|entry| entry.file_name().into_string().unwrap())
        .collect();
    found.sort();
    let mut states = STATES.to_vec();
    states.sort();
    assert_eq!(found, states);
}
