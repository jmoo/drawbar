pub mod common;

use std::io::{BufReader, Read};

use nom_locate::LocatedSpan;

use crate::common::header::{Header, Preamble, Location};
type Span<'a> = LocatedSpan<&'a [u8]>;

fn main() {
    println!("Hello, world!");
    let test = b"\xff\xff\xff\xff";
    let bytes = b"CBIN\x01\x00\x00\x00ne5p\x00\x00\x00\x00\xff\xff\xff\xff";
    let input = Span::new(bytes);

    let header = Header::<2, 3>::parse(input);
    println!("{:?}", header);
    // println!("{:?}", Preamble::parse(input));
    // println!("{:?}", Location::<2, 3>::parse(input));
    // println!("{:?}", Header::<2, 3>::parse(input));

    // Write preamble to buffer
    let mut buffer = [0; 102];
    // let preamble = Preamble {
    //     version: 1,
    //     format: Format(0x70656E33),ß
    // };

    let mut reader = BufReader::new(header.unwrap().1);
    reader.read(buffer.as_mut()).unwrap();

    println!("{:x?}", buffer);
}
