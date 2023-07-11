use std::fs::File;
use std::io::BufReader;
use std::io::Cursor;
use std::io::Read;

use libnord::cbin::FromReader;
use libnord_derive::cbin;

const cbin_header: [u8; 20] = [
    0b01000011, 0b01000010, 0b01001001, 0b01001110, // magic string (CBIN)
    0b00000001, 0b00000000, 0b00000000, 0b00000000, // file veresion (1)
    0b01101110, 0b01100101, 0b00110101, 0b01110000, // file type (ne5p)
    0b00000111, 0b00000000, 0b00000011, 0b00000000, // bank location (7,3)
    0b11111111, 0b11111111, 0b11111111, 0b11111111, // trailer
];

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
pub struct TestProgramHeader {}

#[test]
fn test_parse_cbin_header_from_reader() {
    let mut reader = BufReader::new(Cursor::new(cbin_header.clone()));
    let test = TestProgramHeader::from_reader(&mut reader).unwrap();
    assert_eq!(test.location, (7, 3));
}

#[cbin(format = "ne5p", bank_count = 8, slot_count = 50)]
#[derive(Default)]
pub struct TestBitOffsets {
    #[cbin(bits = 3)]
    pub a: [u8; 1],

    #[cbin(bits = 3)]
    pub b: [u8; 1],

    pub c: u8,

    #[cbin(bits = 2)]
    pub d: [u8; 1],

    #[cbin(bits = 14, from = u16::from_be_bytes)]
    pub e: u16,

    #[cbin(bytes = 1, bits = 2, from = u16::from_be_bytes)]
    pub f: u16,

    #[cbin(bytes = 1)]
    pub g: [u8; 1],
}

#[test]
fn test_bit_offsets() {
    let mut bytes = cbin_header.to_vec();

    bytes.append(&mut vec![
        //aaabbbcc
        0b11001100,

        //ccccccdd
        0b11111101,

        //eeeeeeee
        0b01101100,

        //eeeeeeff
        0b11100111,

        //ffffffff
        0b01011001,

        //gggggggg
        0b00000000,
    ]);

    let test =
        <TestBitOffsets as ::libnord::cbin::FromBytes<TestBitOffsets>>::from_bytes(&bytes).unwrap();

    assert_eq!(
        test.a[0], 0b110,
        "test.a: fetching bits (0b{:03b} != 0b{:03b})",
        test.a[0], 0b110
    );
    assert_eq!(
        test.b[0], 0b011,
        "test.b: fetching bits with an offset (0b{:03b} != 0b{:03b})",
        test.b[0], 0b011
    );
    assert_eq!(
        test.c, 0b00111111,
        "test.c: fetching bytes with an offset (0b{:08b} != 0b{:08b})",
        test.c, 0b00111111
    );
    assert_eq!(
        test.d[0], 0b01,
        "test.d: fetching bits with an offset and zero remaining offset (0b{:02b} != 0b{:02b})",
        test.d[0], 0b01
    );

    assert_eq!(
        test.e, 0b0001101100111001,
        "test.e: fetching bits and bytes (0b{:016b} != 0b{:016b})",
        test.e, 0b01101100111001
    );

    assert_eq!(
        test.f, 0b1101011001,
        "test.f: fetching bits and bytes with an offset (0b{:016b} != 0b{:016b})",
        test.f, 0b1101011001
    );
    
    assert_eq!(test.g[0], 0, "test.g: sanity check to make sure no '1' bits are left in the buffer (0b{:08b} != 0b{:08b})", test.g[0], 0);
}

#[cbin(format = "ne5p", bank_count = 8, slot_count = 50)]
#[derive(Default)]
pub struct TestHangingBits {
    #[cbin(bits = 3)]
    pub a: [u8; 1],
}

#[test]
fn test_hanging_bits_without_panic() {
    let mut bytes = cbin_header.to_vec();

    bytes.append(&mut vec![0b00110011]);

    let test = <TestHangingBits as ::libnord::cbin::FromBytes<TestHangingBits>>::from_bytes(&bytes)
        .unwrap();
    assert_eq!(test.a[0], 0b001, "0b{:03b} != 0b{:03b}", test.a[0], 0b001);
}

#[cbin(fragment)]
#[derive(Default)]
pub struct TestChild {
    pub a: u8,
}

#[cbin(format = "ne5p", bank_count = 8, slot_count = 50)]
#[derive(Default)]
pub struct TestNested {
    #[cbin(bytes = 1)]
    pub child: TestChild,
}

#[test]
fn test_nested() {
    let mut bytes = cbin_header.to_vec();

    bytes.append(&mut vec![0b00110011]);

    let test = <TestNested as ::libnord::cbin::FromBytes<TestNested>>::from_bytes(&bytes).unwrap();
    assert_eq!(
        test.child.a, 0b00110011,
        "0b{:03b} != 0b{:03b}",
        test.child.a, 0b00110011
    );
}
