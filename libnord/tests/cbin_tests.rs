use std::fs::File;
use std::io::Read;
use std::io::BufReader;

use libnord::cbin::FromReader;
use libnord_derive::cbin;

#[cbin]
#[derive(Default)]
pub struct CBinFile {
    #[cbin(
        bytes = 4,
        from = | x: [u8; 4] | String::from_utf8_lossy(&x).to_string()
    )]
    pub cbin: String,

    #[cbin(bits = 3)]
    pub a: [u8; 1],

    // #[cbin(bits = 3)]
    // pub b: u8,

    // #[cbin(bits = 2)]
    // pub c: u8,

    // #[cbin(bits = 7)]
    // pub d: u8,
}

#[test]
fn test_sanity() {
    const TEST_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/ne5/", "song.ne5t");

    let file = File::open(TEST_FILE).unwrap();
    let mut reader = BufReader::new(file);

    let cbin = CBinFile::from_reader(&mut reader).unwrap();

    assert_eq!(cbin.cbin, "CBIN");
    // assert_eq!(cbin.b, 0b000);
    // assert_eq!(cbin.c, 0b11);
    // assert_eq!(cbin.d, 0b0100001);
}
