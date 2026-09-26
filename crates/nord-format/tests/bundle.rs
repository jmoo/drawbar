//! Reading an Electro 5 bundle: where each member goes, and how a member the reader
//! cannot place affects the rest.
#![cfg(feature = "bundle")]

use nord_format::cbin::{Cbin, Header, RawBody};
use nord_format::formats::ne5;
use nord_format::{Entity, Program};
use std::io::Write;

/// A stored archive of `members`; a name ending in `/` becomes a directory entry.
fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in members {
        match name.strip_suffix('/') {
            Some(directory) => zip.add_directory(directory, stored).unwrap(),
            None => {
                zip.start_file(*name, stored).unwrap();
                zip.write_all(bytes).unwrap();
            }
        }
    }
    zip.finish().unwrap().into_inner()
}

fn program(slot: u16) -> Vec<u8> {
    let file = ne5::program::new((0, slot).try_into().unwrap());
    nord_format::to_bytes(&Entity::Program(Program::Electro5(file))).unwrap()
}

/// A valid file of another model, which decodes to an entity a bundle has no place for.
fn drum_program() -> Vec<u8> {
    let file = Cbin {
        header: Header::new("nd2p", (0, 0), 1),
        body: RawBody(vec![0x5a; 16]),
    };
    let mut out = std::io::Cursor::new(Vec::new());
    file.write_to(&mut out).unwrap();
    out.into_inner()
}

#[test]
fn programs_land_in_the_bank_and_a_stray_member_is_accounted_for() {
    let bytes = archive(&[
        ("Bank 1/One.ne5p", &program(0)),
        ("Bank 1/Two.ne5p", &program(1)),
        ("README.txt", b"not a nord file"),
    ]);
    let bundle = ne5::Bundle::read_from(&mut std::io::Cursor::new(bytes)).unwrap();
    let one: ne5::program::Location = (0, 0).try_into().unwrap();
    let two: ne5::program::Location = (0, 1).try_into().unwrap();
    assert!(bundle.programs().get(one).is_some());
    assert!(bundle.programs().get(two).is_some());
    assert_eq!(
        bundle.programs().get(one).unwrap().name.as_deref(),
        Some("Bank 1/One.ne5p"),
        "the archive member's name is the only name a bundle has for an entry"
    );
    assert_eq!(bundle.skipped().len(), 1);
    assert_eq!(bundle.skipped()[0].0, "README.txt");
    assert!(bundle.songs().is_empty());
    assert!(bundle.pianos().is_empty());
    assert!(bundle.samples().is_empty());
}

#[test]
fn an_empty_archive_is_an_empty_bundle() {
    let bundle = ne5::Bundle::read_from(&mut std::io::Cursor::new(archive(&[]))).unwrap();
    assert!(bundle.programs().is_empty());
    assert!(bundle.skipped().is_empty());
}

/// Real backups contain both. Reporting them as skipped would make every backup look
/// partly unreadable.
#[test]
fn a_directory_entry_and_the_manifest_are_not_skipped_members() {
    let bytes = archive(&[
        ("Bank 1/", b""),
        ("meta.xml", b"<meta/>"),
        ("Bank 1/One.ne5p", &program(0)),
    ]);
    let bundle = ne5::Bundle::read_from(&mut std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(bundle.programs().len(), 1);
    assert_eq!(
        bundle.skipped(),
        [] as [(String, String); 0],
        "a directory entry or the manifest was reported as a member"
    );
}

/// Of two members addressed to one slot, the bank keeps the later one.
#[test]
fn a_member_displaced_from_its_slot_is_reported_with_the_member_that_took_it() {
    let bytes = archive(&[
        ("Bank 1/First.ne5p", &program(0)),
        ("Bank 1/Second.ne5p", &program(0)),
    ]);
    let bundle = ne5::Bundle::read_from(&mut std::io::Cursor::new(bytes)).unwrap();
    let at: ne5::program::Location = (0, 0).try_into().unwrap();
    assert_eq!(bundle.programs().len(), 1);
    assert_eq!(
        bundle.programs().get(at).unwrap().name.as_deref(),
        Some("Bank 1/Second.ne5p"),
    );
    assert_eq!(bundle.skipped().len(), 1);
    let (name, why) = &bundle.skipped()[0];
    assert_eq!(name, "Bank 1/First.ne5p");
    assert!(
        why.contains("Bank 1/Second.ne5p"),
        "the reason does not name the member that took the slot: {why}"
    );
}

/// A corrupt member does not fail the whole bundle.
#[test]
fn a_member_with_a_bad_checksum_is_skipped_by_name() {
    let mut corrupt = program(0);
    *corrupt.last_mut().unwrap() ^= 0xff;
    let bytes = archive(&[("Bank 1/One.ne5p", &corrupt)]);
    let bundle = ne5::Bundle::read_from(&mut std::io::Cursor::new(bytes)).unwrap();
    assert!(bundle.programs().is_empty());
    assert_eq!(bundle.skipped().len(), 1);
    let (name, why) = &bundle.skipped()[0];
    assert_eq!(name, "Bank 1/One.ne5p");
    assert!(
        why.contains("checksum"),
        "the reason does not mention the checksum: {why}"
    );
}

/// A file of another model decodes, so its reason differs from a decode failure.
#[test]
fn a_member_of_another_model_is_skipped_as_having_no_place() {
    let bytes = archive(&[("One.nd2p", &drum_program())]);
    let bundle = ne5::Bundle::read_from(&mut std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(bundle.skipped().len(), 1);
    assert_eq!(
        bundle.skipped()[0].1,
        "no place in a bundle for a Drum 2 program"
    );
}

/// A ZIP error fails the whole read: half an archive has no members to walk, so there is
/// no partial bundle to report.
#[test]
fn a_truncated_archive_fails_the_read() {
    let bytes = archive(&[("Bank 1/One.ne5p", &program(0))]);
    let half = &bytes[..bytes.len() / 2];
    assert!(ne5::Bundle::read_from(&mut std::io::Cursor::new(half)).is_err());
}
