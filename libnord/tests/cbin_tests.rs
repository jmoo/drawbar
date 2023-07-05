use std::fs::File;
use std::io::Read;
use std::io::BufReader;

use libnord::cbin::FromReader;
use libnord_derive::cbin;

// Todo:
//   - remove need for dervive(Default) because we can just assign everything to the struct all at once
//   - implement default mappers for primitive types
//   - dont require cbin attribute for primitive types
//   - implement padding
//   - implement ability to nest schemas
//   - implement crc checking (seems to be part of the cbin format so it might make sense to implement it entirely in macro)
//   - implement writing
//   - remove need to import std::io::read
//   - change format and magic strings to byte strings: b"CBIN"


#[cbin(format = "ne5p", bank_count = 8, slot_count = 50)]
#[derive(Default)]
pub struct TestProgram {

}

#[test]
fn test_sanity() {
    const TEST_FILE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/resources/ne5/programs/center_panel/o00_1_p000_0_1_0_50_50.ne5p"
    );

    let file = File::open(TEST_FILE).unwrap();
    let mut reader = BufReader::new(file);

    let test = TestProgram::from_reader(&mut reader).unwrap();

    assert_eq!(test.location, (7, 3));
}
