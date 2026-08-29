#[path = "support/format_table.rs"]
mod format_table;

use nord_format::cbin::{Body, Cbin, Generation, Header, RawBody};
use nord_format::formats::ne5;
use nord_format::formats::nsmpproj::{NewZone, Project};
use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

fn normalized_project(bytes: &[u8]) -> String {
    let text = std::str::from_utf8(bytes).unwrap();
    let marker = "m_createdByProdVer = ";
    let start = text.find(marker).unwrap() + marker.len();
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |end| start + end);
    format!("{}<crate version>{}", &text[..start], &text[end..])
}

fn written<B: Body>(file: &Cbin<B>) -> Vec<u8> {
    let mut bytes = Vec::new();
    file.write_to(&mut Cursor::new(&mut bytes)).unwrap();
    bytes
}

#[test]
fn fresh_ne5_writers_match_reviewed_minimal_fixtures() {
    let slot = |bank, program| (bank, program).try_into().unwrap();
    let program = ne5::program::new(slot(0, 0));
    let live = ne5::live::new((0, 0).try_into().unwrap());
    let settings = ne5::settings::new();
    let song = ne5::song::new(
        (0, 2).try_into().unwrap(),
        1,
        [slot(5, 9), slot(0, 1), slot(0, 2), slot(5, 8)],
    );

    assert_eq!(written(&program), fixture("ne5/default.ne5p"));
    assert_eq!(written(&live), fixture("ne5/default.ne5l"));
    assert_eq!(written(&settings), fixture("ne5/default.ne5s"));
    assert_eq!(written(&song), fixture("ne5/song.ne5t"));
}

#[test]
fn a_minimal_sample_project_matches_the_reviewed_fixture() {
    let project = Project::new(
        "one-zone",
        &[NewZone {
            path: "audio/c4.wav".into(),
            sample_rate: 44_100,
            frames: 4_394,
            root_key: 60,
        }],
        1_700_000_000,
    )
    .unwrap();

    let expected = fixture("nsmpproj/one-zone.nsmpproj");
    assert_eq!(
        normalized_project(project.render().as_bytes()),
        normalized_project(&expected),
    );
}

#[test]
fn every_registered_cbin_writer_matches_both_reviewed_container_generations() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cbin");
    let mut expected = BTreeSet::new();
    for (tag, body_len, version) in format_table::formats() {
        for (number, generation) in [(0, Generation::V0), (1, Generation::V1)] {
            let name = format!("{}.g{number}.cbin", tag.trim_end_matches('\0'));
            let mut header = Header::new(tag, (0, 0), version);
            header.generation = generation;
            let file = Cbin {
                header,
                body: RawBody(vec![0; body_len as usize]),
            };
            assert_eq!(written(&file), fixture(&format!("cbin/{name}")), "{name}");
            expected.insert(name);
        }
    }
    let committed = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(committed, expected);
}
