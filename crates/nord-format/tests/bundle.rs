//! The Electro 5 bundle walk: what lands where, and what a member the walk cannot
//! place does to the rest.
#![cfg(feature = "bundle")]

use nord_format::formats::ne5;
use nord_format::{Entity, Program};
use std::io::Write;

fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in members {
        zip.start_file(*name, stored).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn program(slot: u16) -> Vec<u8> {
    let file = ne5::program::new((0, slot).try_into().unwrap());
    nord_format::to_bytes(&Entity::Program(Program::Electro5(file))).unwrap()
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
