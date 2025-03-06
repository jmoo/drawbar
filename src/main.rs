pub mod common;
pub mod electro5;

use std::env;

use nom_locate::LocatedSpan;

use crate::common::entity::Entity;
use memmap::Mmap;
use std::fs::File;

type Span<'a> = LocatedSpan<&'a [u8]>;

fn main() {
    for arg in env::args().skip(1) {
        let file = File::open(arg).unwrap();
        let mmap = unsafe { Mmap::map(&file).unwrap() };
        let bytes = &mmap[..];
        let input = Span::new(&bytes);
        let (_, entity) = Entity::parse(input).unwrap();
        println!("{:?}", entity);
    }
}
