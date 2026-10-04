//! A bundle's container and manifest: what the writer puts down, what the reader
//! refuses, and how a new bundle is laid out.

use nord_format::bundle::archive::{copy_member, Directory, DosTime, Entry, Frame, Writer};
use nord_format::bundle::manifest::{Dependencies, Manifest};
use nord_format::bundle::{electro5_path, Class, Item, Key, Plan};
use nord_format::cbin::Header;
use std::io::{Cursor, Write};

const MODIFIED: DosTime = DosTime {
    time: 0x860d,
    date: 0x5cf9,
};

fn entry(name: &str, body: &[u8]) -> Entry {
    let size = u32::try_from(body.len()).unwrap();
    Entry::new(name.into(), size, nord_format::crc::crc32(body), MODIFIED)
}

fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new());
    for (name, body) in members {
        writer.member(entry(name, body), &mut &body[..]).unwrap();
    }
    writer.finish(&[]).unwrap()
}

fn read(bytes: &[u8]) -> Result<Directory, String> {
    Directory::read_from(&mut Cursor::new(bytes)).map_err(|e| e.to_string())
}

#[test]
fn a_dos_time_packs_as_the_words_an_nsm_member_recorded() {
    // A member NSM wrote at 2026-07-25 16:48:26 carries time 0x860d, date 0x5cf9.
    assert_eq!(DosTime::new(2026, 7, 25, 16, 48, 26), Some(MODIFIED));
    assert_eq!(
        DosTime::new(1979, 12, 31, 0, 0, 0),
        None,
        "before the epoch"
    );
    assert_eq!(DosTime::new(2026, 13, 1, 0, 0, 0), None, "month 13");
}

/// The bytes of a one-member stored archive, laid out field by field from APPNOTE
/// 4.3.7, 4.3.12 and 4.3.16 as NSM fills them.
#[test]
fn one_member_is_written_as_the_zip_specification_lays_it_out() {
    let crc = nord_format::crc::crc32(b"abc").to_le_bytes();
    let mut expected = Vec::new();
    // Local header: signature, version 2.0, no flags, stored, time, date.
    expected.extend([
        0x50, 0x4b, 0x03, 0x04, 0x14, 0, 0, 0, 0, 0, 0x0d, 0x86, 0xf9, 0x5c,
    ]);
    expected.extend(crc);
    expected.extend([3, 0, 0, 0, 3, 0, 0, 0, 5, 0, 0, 0]);
    expected.extend(b"a.txt");
    expected.extend(b"abc");
    // Central header: made by MS-DOS 3.2, the same fields, no extra or comment, offset 0.
    expected.extend([0x50, 0x4b, 0x01, 0x02, 0x20, 0, 0x14, 0, 0, 0, 0, 0]);
    expected.extend([0x0d, 0x86, 0xf9, 0x5c]);
    expected.extend(crc);
    expected.extend([
        3, 0, 0, 0, 3, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    expected.extend([0, 0, 0, 0]);
    expected.extend(b"a.txt");
    // End record: one entry, a 51-byte directory at offset 38, no comment.
    expected.extend([0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 1, 0, 1, 0]);
    expected.extend([51, 0, 0, 0, 38, 0, 0, 0, 0, 0]);

    assert_eq!(archive(&[("a.txt", b"abc")]), expected);
}

#[test]
fn a_written_archive_reads_back_with_each_member_at_its_range() {
    let bytes = archive(&[("one", b"first"), ("dir/two", b""), ("three", b"third!")]);
    let directory = read(&bytes).unwrap();
    let found: Vec<(&str, &[u8])> = directory
        .members
        .iter()
        .map(|m| {
            let body = &bytes[m.body.start as usize..m.body.end as usize];
            (m.entry.name.as_str(), body)
        })
        .collect();
    assert_eq!(
        found,
        [
            ("one", &b"first"[..]),
            ("dir/two", b""),
            ("three", b"third!")
        ]
    );
}

#[test]
fn a_frame_is_the_bytes_a_writer_puts_around_the_members() {
    let members: [(&str, &[u8]); 2] = [("one", b"first"), ("two", b"second")];
    let entries: Vec<Entry> = members
        .iter()
        .map(|(name, body)| entry(name, body))
        .collect();
    let frame = Frame::new(&entries, &[]).unwrap();
    let mut assembled: Vec<u8> = Vec::new();
    for (header, (_, body)) in frame.headers.iter().zip(members) {
        assembled.extend(header);
        assembled.extend(body);
    }
    assembled.extend(&frame.trailer);
    assert_eq!(assembled, archive(&members));
}

#[test]
fn a_non_ascii_name_is_flagged_utf8_and_reads_back() {
    let bytes = archive(&[("Grand Piano é.npno", b"x")]);
    let directory = read(&bytes).unwrap();
    assert_eq!(directory.members[0].entry.name, "Grand Piano é.npno");
    assert_eq!(directory.members[0].entry.verbatim.flags, 0x0800);
}

#[test]
fn the_writer_refuses_bytes_that_disagree_with_the_entry() {
    let mut writer = Writer::new(Vec::new());
    writer.begin(entry("a", b"abc")).unwrap();
    assert!(writer.write_all(b"abcd").is_err(), "a byte past the size");

    let mut writer = Writer::new(Vec::new());
    writer.begin(entry("a", b"abc")).unwrap();
    writer.write_all(b"ab").unwrap();
    assert!(writer.finish(&[]).is_err(), "a member one byte short");

    let mut writer = Writer::new(Vec::new());
    writer.begin(entry("a", b"abc")).unwrap();
    writer.write_all(b"abd").unwrap();
    assert!(writer.finish(&[]).is_err(), "bytes with another CRC");
}

#[test]
fn copying_a_member_checks_its_crc() {
    let mut bytes = archive(&[("a", b"abc")]);
    let directory = read(&bytes).unwrap();
    let member = &directory.members[0];
    let mut out = Vec::new();
    copy_member(&mut Cursor::new(&bytes), member, &mut out).unwrap();
    assert_eq!(out, b"abc");

    bytes[member.body.start as usize] ^= 1;
    let copied = copy_member(&mut Cursor::new(&bytes), member, &mut Vec::new());
    assert!(copied.unwrap_err().to_string().contains("CRC-32"));
}

/// Offsets into [`one_member`]'s bytes.
const LOCAL_FLAGS: usize = 6;
const LOCAL_METHOD: usize = 8;
const CENTRAL_AT: usize = 30 + 1 + 3;

fn one_member() -> Vec<u8> {
    archive(&[("a", b"abc")])
}

#[test]
fn only_nsm_shaped_archives_are_read() {
    let refused = |what: &str, bytes: Vec<u8>| {
        assert!(read(&bytes).is_err(), "{what} was read");
    };

    let mut compressed = one_member();
    compressed[LOCAL_METHOD] = 8;
    compressed[CENTRAL_AT + 10] = 8;
    refused("a deflated member", compressed);

    let mut described = one_member();
    described[LOCAL_FLAGS] = 0x08;
    described[CENTRAL_AT + 8] = 0x08;
    refused("a member with a data descriptor", described);

    let mut mismatched = one_member();
    mismatched[LOCAL_FLAGS] = 0x08;
    refused("a local header the directory does not describe", mismatched);

    let mut trailing = one_member();
    trailing.push(0);
    refused("a byte after the end record", trailing);

    let mut gapped = one_member();
    gapped.insert(CENTRAL_AT, 0);
    refused("a byte between the last member and the directory", gapped);

    let bytes = one_member();
    refused("a truncated archive", bytes[..bytes.len() - 1].to_vec());
    refused("an empty file", Vec::new());
}

#[test]
fn an_archive_comment_reads_back() {
    let mut writer = Writer::new(Vec::new());
    writer.member(entry("a", b"abc"), &mut &b"abc"[..]).unwrap();
    let bytes = writer.finish(b"made here").unwrap();
    assert_eq!(read(&bytes).unwrap().comment, b"made here");
}

#[cfg(feature = "bundle")]
#[test]
fn another_zip_reader_reads_what_the_writer_wrote() {
    use std::io::Read;
    let bytes = archive(&[("one", b"first"), ("Grand é", b"second")]);
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut names = Vec::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).unwrap();
        let mut body = String::new();
        file.read_to_string(&mut body).unwrap();
        names.push((file.name().to_string(), body));
    }
    assert_eq!(
        names,
        [
            ("one".into(), "first".into()),
            ("Grand é".into(), "second".into())
        ]
    );
}

const MANIFEST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<bundle version=\"1\" product=\"39\" product_version=\"204\" content_version=\"1\" source=\"-1\">
  <file name=\"Set List/Set List 1/R&amp;B.ne5t\" depCnt=\"2\" dep0=\"Program/Bank 5/One.ne5p\" dep1=\"Program/Bank 5/Two's.ne5p\"/>
  <file name=\"Program/Bank 5/One.ne5p\" depCnt=\"1\" dep0=\"Piano/Grand/A &lt;b&gt; &quot;c&quot;.npno\"/>
</bundle>
";

#[test]
fn a_manifest_reads_its_dependencies_and_writes_back_the_same() {
    let manifest = Manifest::parse(MANIFEST.as_bytes()).unwrap();
    assert_eq!(
        manifest,
        Manifest::electro5(
            204,
            vec![
                Dependencies {
                    name: "Set List/Set List 1/R&B.ne5t".into(),
                    deps: vec![
                        "Program/Bank 5/One.ne5p".into(),
                        "Program/Bank 5/Two's.ne5p".into()
                    ],
                },
                Dependencies {
                    name: "Program/Bank 5/One.ne5p".into(),
                    deps: vec!["Piano/Grand/A <b> \"c\".npno".into()],
                },
            ]
        )
    );
    assert_eq!(manifest.to_bytes(), MANIFEST.as_bytes());
    assert_eq!(
        manifest.deps("Piano/Grand/A <b> \"c\".npno"),
        [] as [String; 0]
    );
}

#[test]
fn a_manifest_in_any_other_shape_is_refused() {
    let refused = |what: &str, text: String| {
        assert!(Manifest::parse(text.as_bytes()).is_err(), "{what} was read");
    };
    refused(
        "a depCnt above its deps",
        MANIFEST.replace("depCnt=\"1\"", "depCnt=\"2\""),
    );
    refused(
        "a depCnt below its deps",
        MANIFEST.replace("depCnt=\"2\"", "depCnt=\"1\""),
    );
    refused(
        "a number with a leading zero",
        MANIFEST.replace("\"204\"", "\"0204\""),
    );
    refused(
        "an unknown attribute",
        MANIFEST.replace("source=", "origin="),
    );
    refused("a bare ampersand", MANIFEST.replace("&amp;", "&"));
    refused(
        "a missing declaration",
        MANIFEST.replace("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n", ""),
    );
    refused(
        "a trailing element",
        MANIFEST.replace("</bundle>\n", "</bundle>\n<x/>\n"),
    );
    refused(
        "tabs for indentation",
        MANIFEST.replace("  <file", "\t<file"),
    );
}

fn header(tag: &str, bank: u16, slot: u16) -> Header {
    Header::new(tag, (bank, slot), 1)
}

#[test]
fn a_member_path_is_its_class_its_bank_and_its_name() {
    let path = |tag, bank| electro5_path(&header(tag, bank, 3), "Name");
    assert_eq!(path("ne5p", 6).as_deref(), Some("Program/Bank 7/Name.ne5p"));
    assert_eq!(
        path("ne5t", 0).as_deref(),
        Some("Set List/Set List 1/Name.ne5t")
    );
    assert_eq!(path("npno", 5).as_deref(), Some("Piano/Harps/Name.npno"));
    assert_eq!(
        path("nsmp", 0).as_deref(),
        Some("Samp Lib/Samp Lib/Name.nsmp")
    );
    assert_eq!(path("npno", 6), None, "a seventh piano bank");
    assert_eq!(path("ne6p", 0), None, "another model's program");
    assert_eq!(electro5_path(&header("ne5p", 0, 0), "a/b"), None);
}

fn item(path: &str, class: Class, provides: Option<Key>, needs: &[Key]) -> Item {
    Item {
        path: path.into(),
        class,
        provides,
        needs: needs.to_vec(),
    }
}

#[test]
fn a_plan_stores_libraries_first_and_lists_set_lists_first() {
    let items = vec![
        item(
            "S.ne5t",
            Class::SetList,
            None,
            &[Key::Program(0, 1), Key::Program(0, 1), Key::Program(0, 2)],
        ),
        item(
            "B.ne5p",
            Class::Program,
            Some(Key::Program(0, 2)),
            &[Key::Piano(7)],
        ),
        item(
            "A.ne5p",
            Class::Program,
            Some(Key::Program(0, 1)),
            &[Key::Piano(7), Key::Sample(9)],
        ),
        item("P.npno", Class::Piano, Some(Key::Piano(7)), &[]),
        item("Q.nsmp", Class::Sample, None, &[]),
    ];
    let plan = Plan::new(items, 204).unwrap();
    let order: Vec<&str> = plan.members.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(order, ["P.npno", "Q.nsmp", "B.ne5p", "A.ne5p", "S.ne5t"]);
    assert_eq!(
        plan.manifest.files,
        [
            Dependencies {
                name: "S.ne5t".into(),
                deps: vec!["A.ne5p".into(), "B.ne5p".into()]
            },
            Dependencies {
                name: "B.ne5p".into(),
                deps: vec!["P.npno".into()]
            },
            Dependencies {
                name: "A.ne5p".into(),
                deps: vec!["P.npno".into()]
            },
        ]
    );
    assert_eq!(plan.unmet, [("A.ne5p".to_string(), Key::Sample(9))]);
}

#[test]
fn a_plan_refuses_two_members_at_one_path() {
    let twice = vec![
        item("A.ne5p", Class::Program, None, &[]),
        item("A.ne5p", Class::Program, None, &[]),
    ];
    assert!(Plan::new(twice, 204).is_err());
}

#[test]
fn a_unix_time_becomes_the_utc_dos_time() {
    // 0x5e98c95a is 2020-04-16 21:08:42 UTC.
    assert_eq!(
        DosTime::from_unix(0x5e98_c95a),
        DosTime::new(2020, 4, 16, 21, 8, 42)
    );
    assert_eq!(
        DosTime::from_unix(951_782_400),
        DosTime::new(2000, 2, 29, 0, 0, 0)
    );
    assert_eq!(DosTime::from_unix(0), None, "1970 is before the DOS epoch");
}
