//! Reading an Electro 5 bundle: where each member goes, and how a member the reader
//! cannot place affects the rest.
#![cfg(feature = "bundle")]

use nord_format::bundle::manifest::{self, Manifest};
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

/// [`archive`] with an empty manifest after `members`.
fn bundle(members: &[(&str, &[u8])]) -> Vec<u8> {
    let manifest = Manifest::electro5(204, Vec::new()).to_bytes();
    let mut all = members.to_vec();
    all.push((manifest::PATH, &manifest));
    archive(&all)
}

fn read(bytes: Vec<u8>) -> Result<ne5::Bundle, String> {
    ne5::Bundle::read_from(&mut std::io::Cursor::new(bytes)).map_err(|e| e.to_string())
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
    let bytes = bundle(&[
        ("Bank 1/One.ne5p", &program(0)),
        ("Bank 1/Two.ne5p", &program(1)),
        ("README.txt", b"not a nord file"),
    ]);
    let bundle = read(bytes).unwrap();
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

/// A bundle holds members and the manifest that describes them, so an archive missing
/// either is not one.
#[test]
fn an_archive_with_no_members_or_no_readable_manifest_is_refused() {
    let one = program(0);
    let empty = Manifest::electro5(204, Vec::new()).to_bytes();
    for (members, why) in [
        (&[][..], "the archive holds no members"),
        (
            &[("meta.xml", &empty[..])][..],
            "the archive holds no members",
        ),
        (
            &[("Bank 1/One.ne5p", &one[..])][..],
            "the archive has no meta.xml",
        ),
        (
            &[("Bank 1/One.ne5p", &one[..]), ("meta.xml", b"<meta/>")][..],
            "meta.xml: a missing XML declaration",
        ),
        (
            &[("Bank 1/One.ne5p", &one[..]), ("meta.xml", b"")][..],
            "meta.xml: a missing XML declaration",
        ),
    ] {
        let names: Vec<_> = members.iter().map(|(name, _)| *name).collect();
        assert_eq!(read(archive(members)).unwrap_err(), why, "{names:?}");
    }
}

/// A backup carries its own manifest in place of a bundle's, and reads as a bundle does.
#[test]
fn a_backup_manifest_stands_in_for_a_bundles() {
    let backup = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<backup file_version=\"1\"/>\n";
    let bytes = archive(&[("Bank 1/One.ne5p", &program(0)), ("meta.xml", backup)]);
    assert_eq!(read(bytes).unwrap().programs().len(), 1);
}

#[test]
fn a_backup_manifest_is_an_xml_declaration_and_one_backup_element() {
    for manifest in [
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<backup file_version=\"1\" backup_format_version=\"1\" product_id=\"41\"/>\n",
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<backup/>\n",
        "<?xml version=\"1.0\"?>\n<backup file_version=\"1\"/>\n",
    ] {
        assert!(manifest::is_backup(manifest.as_bytes()), "{manifest:?}");
    }
}

#[test]
fn a_malformed_backup_manifest_is_refused() {
    for manifest in [
        "<?xml bogus?>\n<backup broken/>\n",
        "<?xml version=\"1.0\"?>\n<backup broken/>\n",
        "<?xml bogus?>\n<backup/>\n",
        "<?xml version=\"2.0\"?>\n<backup/>\n",
        "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\n<backup/>\n",
        "<?xml encoding=\"UTF-8\" version=\"1.0\"?>\n<backup/>\n",
        "<backup/>\n",
        "<?xml version=\"1.0\"?>\n<bundle/>\n",
        "<?xml version=\"1.0\"?>\n<backups/>\n",
        "<?xml version=\"1.0\"?>\n<backup a=\"1\"/>\n<backup a=\"2\"/>\n",
        "<?xml version=\"1.0\"?>\n<backup a=\"1\">\n",
        "<?xml version=\"1.0\"?>\n<backup a=\"1/>\n",
        "<?xml version=\"1.0\"?>\n<backup a=\"<\"/>\n",
        "<?xml version=\"1.0\"?>\n<backup/>\ntrailing",
    ] {
        assert!(!manifest::is_backup(manifest.as_bytes()), "{manifest:?}");
    }
}

/// `nord inspect` reads a bundle through this check, and `nord bundle unpack` runs it.
#[test]
fn an_archive_whose_backup_manifest_is_malformed_is_refused() {
    let broken = b"<?xml bogus?>\n<backup broken/>\n";
    let bytes = archive(&[("Bank 1/One.ne5p", &program(0)), ("meta.xml", broken)]);
    assert!(read(bytes).is_err(), "a malformed backup manifest was read");
}

/// The manifest describes the archive. Reporting it as skipped would make every bundle
/// look partly unreadable.
#[test]
fn the_manifest_is_not_a_skipped_member() {
    let bytes = bundle(&[("Bank 1/One.ne5p", &program(0))]);
    let bundle = read(bytes).unwrap();
    assert_eq!(bundle.programs().len(), 1);
    assert_eq!(
        bundle.skipped(),
        [] as [(String, String); 0],
        "the manifest was reported as a member"
    );
}

/// The archive layer's refusals hold for the decoded bundle too, so a bundle that
/// `nord inspect` reads is one an unpack accepts.
#[test]
fn an_archive_the_bundle_reader_refuses_fails_the_read() {
    let refused = |members: &[(&str, &[u8])]| {
        ne5::Bundle::read_from(&mut std::io::Cursor::new(archive(members)))
            .unwrap_err()
            .to_string()
    };
    let one = program(0);
    assert_eq!(
        refused(&[("Bank 1/", b""), ("Bank 1/One.ne5p", &one)]),
        "the entry \"Bank 1/\" is a folder, not a file"
    );
    assert_eq!(
        refused(&[("../One.ne5p", &one)]),
        "member \"../One.ne5p\" is not a path inside the bundle"
    );

    let mut lying = bundle(&[("Bank 1/One.ne5p", &one)]);
    let body = 30 + "Bank 1/One.ne5p".len();
    lying[body] ^= 0xff;
    let err = read(lying).unwrap_err();
    assert!(
        err.contains("CRC-32"),
        "refused for the wrong reason: {err}"
    );
}

/// Of two members addressed to one slot, the bank keeps the later one.
#[test]
fn a_member_displaced_from_its_slot_is_reported_with_the_member_that_took_it() {
    let bytes = bundle(&[
        ("Bank 1/First.ne5p", &program(0)),
        ("Bank 1/Second.ne5p", &program(0)),
    ]);
    let bundle = read(bytes).unwrap();
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
    let bytes = bundle(&[("Bank 1/One.ne5p", &corrupt)]);
    let bundle = read(bytes).unwrap();
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
    let bytes = bundle(&[("One.nd2p", &drum_program())]);
    let bundle = read(bytes).unwrap();
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
    let bytes = bundle(&[("Bank 1/One.ne5p", &program(0))]);
    let half = bytes[..bytes.len() / 2].to_vec();
    assert!(read(half).is_err());
}
