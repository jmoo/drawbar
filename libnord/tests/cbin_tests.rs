use std::fs::File;
use std::io::BufReader;

use libnord::cbin::FromReader;
use libnord_derive::cbin;

#[cbin]
#[derive(Default)]
pub struct CBinFile {
    #[cbin(bits = 3)]
    pub a: u8,

    #[cbin(bits = 3)]
    pub b: u8,

    #[cbin(bits = 2)]
    pub c: u8,

    #[cbin(bits = 7)]
    pub d: u8,
}

#[test]
fn test_sanity() {
    const TEST_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/ne5/", "song.ne5t");

    let file = File::open(TEST_FILE).unwrap();
    let mut reader = BufReader::new(file);

    let cbin = CBinFile::from_reader(&mut reader).unwrap();

    assert_eq!(cbin.a, 0b010);
    assert_eq!(cbin.b, 0b000);
    assert_eq!(cbin.c, 0b11);
    assert_eq!(cbin.d, 0b0100001);
}
